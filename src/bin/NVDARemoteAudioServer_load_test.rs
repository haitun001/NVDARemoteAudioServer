use std::env;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use nvdaremoteaudio_server::config::{
    Config, HANDSHAKE_MAX_BYTES, HANDSHAKE_TIMEOUT_MS, STATUS_ACCESS_KEY,
    STATUS_RESPONSE_MAX_BYTES, TCP_HEARTBEAT_INTERVAL_MS, UDP_AUDIO_PAYLOAD_MAX_BYTES,
    UDP_PACKET_MAX_BYTES, UDP_SESSION_TIMEOUT_MS,
};
use nvdaremoteaudio_server::protocol::{
    ClientRole, ControlMessageRequest, ControlMessageType, HandshakeRequest,
    HandshakeResponseOwned, SessionId, StatusAccessRequest, encode_udp_audio_data,
    encode_udp_heartbeat, encode_udp_register, parse_udp_packet, read_json_line, write_json_line,
};
use nvdaremoteaudio_server::server;
use nvdaremoteaudio_server::state::RegistrySnapshot;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::{TcpStream, UdpSocket, lookup_host};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const DEFAULT_PUBLISHERS: usize = 20;
const DEFAULT_SUBSCRIBERS_PER_PUBLISHER: usize = 20;
const DEFAULT_PACKETS_PER_PUBLISHER: usize = 200;
const DEFAULT_PAYLOAD_BYTES: usize = 160;
const DEFAULT_PACKET_INTERVAL_MS: u64 = 10;
const DEFAULT_HEARTBEAT_ROUNDS: usize = 2;
const DEFAULT_HEARTBEAT_ROUND_INTERVAL_MS: u64 = 1_000;
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 6838;
const DEFAULT_STATUS_PORT: u16 = 6839;
const LOAD_TEST_UDP_PUBLISHER_RECV_BUFFER_BYTES: usize = 8 * 1024;
const LOAD_TEST_UDP_PUBLISHER_SEND_BUFFER_BYTES: usize = 16 * 1024;
const LOAD_TEST_UDP_SUBSCRIBER_RECV_BUFFER_BYTES: usize = 32 * 1024;
const LOAD_TEST_UDP_SUBSCRIBER_SEND_BUFFER_BYTES: usize = 8 * 1024;
const SUBSCRIBER_RECV_TIMEOUT_SECS: u64 = 60;

struct LoadTestConfig {
    host: String,
    publishers: usize,
    subscribers_per_publisher: usize,
    packets_per_publisher: usize,
    payload_bytes: usize,
    packet_interval_ms: u64,
    heartbeat_rounds: usize,
    heartbeat_round_interval_ms: u64,
    port: u16,
    status_port: u16,
    external_server: bool,
}

impl LoadTestConfig {
    fn from_args() -> io::Result<Self> {
        let mut config = Self {
            host: DEFAULT_HOST.to_owned(),
            publishers: DEFAULT_PUBLISHERS,
            subscribers_per_publisher: DEFAULT_SUBSCRIBERS_PER_PUBLISHER,
            packets_per_publisher: DEFAULT_PACKETS_PER_PUBLISHER,
            payload_bytes: DEFAULT_PAYLOAD_BYTES,
            packet_interval_ms: DEFAULT_PACKET_INTERVAL_MS,
            heartbeat_rounds: DEFAULT_HEARTBEAT_ROUNDS,
            heartbeat_round_interval_ms: DEFAULT_HEARTBEAT_ROUND_INTERVAL_MS,
            port: DEFAULT_PORT,
            status_port: DEFAULT_STATUS_PORT,
            external_server: false,
        };

        for arg in env::args().skip(1) {
            if let Some(value) = arg.strip_prefix("--host=") {
                if value.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--host must be non-empty",
                    ));
                }
                config.host = value.to_owned();
                continue;
            }
            if let Some(value) = arg.strip_prefix("--publishers=") {
                config.publishers = parse_usize(value, "--publishers")?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--subscribers-per-publisher=") {
                config.subscribers_per_publisher =
                    parse_usize(value, "--subscribers-per-publisher")?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--packets-per-publisher=") {
                config.packets_per_publisher = parse_usize(value, "--packets-per-publisher")?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--payload-bytes=") {
                config.payload_bytes = parse_usize(value, "--payload-bytes")?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--packet-interval-ms=") {
                config.packet_interval_ms = parse_u64(value, "--packet-interval-ms")?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--heartbeat-rounds=") {
                config.heartbeat_rounds = parse_usize(value, "--heartbeat-rounds")?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--heartbeat-round-interval-ms=") {
                config.heartbeat_round_interval_ms =
                    parse_u64(value, "--heartbeat-round-interval-ms")?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--port=") {
                config.port = parse_port(value, "--port")?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--sport=") {
                config.status_port = parse_port(value, "--sport")?;
                continue;
            }
            if arg == "--external-server" {
                config.external_server = true;
                continue;
            }
            if arg == "--help" || arg == "-h" {
                return Err(io::Error::other(
                    "usage: NVDARemoteAudioServer_load_test [--host=127.0.0.1] [--publishers=20] [--subscribers-per-publisher=20] [--packets-per-publisher=200] [--payload-bytes=160] [--packet-interval-ms=10] [--heartbeat-rounds=2] [--heartbeat-round-interval-ms=1000] [--port=6838] [--sport=6839] [--external-server]",
                ));
            }

            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown argument: {arg}"),
            ));
        }

        if config.publishers == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--publishers must be greater than 0",
            ));
        }
        if config.subscribers_per_publisher == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--subscribers-per-publisher must be greater than 0",
            ));
        }
        if config.packets_per_publisher == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--packets-per-publisher must be greater than 0",
            ));
        }
        if config.payload_bytes == 0 || config.payload_bytes > UDP_AUDIO_PAYLOAD_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "--payload-bytes must be between 1 and {}",
                    UDP_AUDIO_PAYLOAD_MAX_BYTES
                ),
            ));
        }

        Ok(config)
    }
}

struct HeartbeatTask {
    task: Option<JoinHandle<io::Result<()>>>,
}

impl HeartbeatTask {
    fn new(task: JoinHandle<io::Result<()>>) -> Self {
        Self { task: Some(task) }
    }

    async fn stop(&mut self, name: &str) -> io::Result<()> {
        let Some(task) = self.task.take() else {
            return Ok(());
        };

        task.abort();
        match task.await {
            Err(err) if err.is_cancelled() => Ok(()),
            Err(err) => Err(io::Error::other(format!(
                "{name} heartbeat task join failed: {err}"
            ))),
            Ok(Err(err)) => Err(io::Error::other(format!(
                "{name} heartbeat task failed: {err}"
            ))),
            Ok(Ok(())) => Err(io::Error::other(format!(
                "{name} heartbeat task exited unexpectedly"
            ))),
        }
    }
}

impl Drop for HeartbeatTask {
    fn drop(&mut self) {
        if let Some(task) = self.task.as_ref() {
            task.abort();
        }
    }
}

struct PublisherClient {
    key_index: usize,
    _control_reader: tokio::net::tcp::OwnedReadHalf,
    session_id: SessionId,
    socket: std::sync::Arc<UdpSocket>,
    control_heartbeat: HeartbeatTask,
    udp_heartbeat: HeartbeatTask,
}

struct SubscriberClient {
    key_index: usize,
    _control_reader: tokio::net::tcp::OwnedReadHalf,
    session_id: SessionId,
    socket: std::sync::Arc<UdpSocket>,
    control_heartbeat: HeartbeatTask,
    udp_heartbeat: HeartbeatTask,
    expected_packets: usize,
    payload_bytes: usize,
    receive_task: Option<JoinHandle<io::Result<SubscriberResult>>>,
}

struct SubscriberResult {
    key_index: usize,
    expected_packets: usize,
    unique_packets_received: usize,
    duplicate_packets: usize,
    invalid_packets: usize,
    missing_packets: usize,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let config = match LoadTestConfig::from_args() {
        Ok(config) => config,
        Err(err) if err.kind() == io::ErrorKind::Other => {
            eprintln!("{err}");
            return Ok(());
        }
        Err(err) => return Err(err),
    };

    let start = Instant::now();
    let control_addr = resolve_socket_addr(&config.host, config.port).await?;
    let status_addr = resolve_socket_addr(&config.host, config.status_port).await?;

    let server_task = if config.external_server {
        None
    } else {
        if !control_addr.ip().is_loopback() || !status_addr.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--host must resolve to a loopback address unless --external-server is set",
            ));
        }
        Some(tokio::spawn(server::run(Config {
            bind_addr: control_addr,
            status_bind_addr: status_addr,
            log_path: None,
        })))
    };

    if server_task.is_some() {
        sleep(Duration::from_millis(150)).await;
    }

    let mut publishers = Vec::with_capacity(config.publishers);
    let mut subscribers = Vec::with_capacity(config.publishers * config.subscribers_per_publisher);
    for key_index in 0..config.publishers {
        let key = stream_key(key_index);
        let publisher = connect_publisher(control_addr, key_index, &key)
            .await
            .map_err(|err| {
                io::Error::other(format!(
                    "connect_publisher failed for key_index={key_index} key={key}: {err}"
                ))
            })?;
        publishers.push(publisher);

        for _ in 0..config.subscribers_per_publisher {
            let subscriber = connect_subscriber(
                control_addr,
                key_index,
                &key,
                config.packets_per_publisher,
                config.payload_bytes,
            )
            .await
            .map_err(|err| {
                io::Error::other(format!(
                    "connect_subscriber failed for key_index={key_index} key={key}: {err}"
                ))
            })?;
            subscribers.push(subscriber);
        }
    }

    start_subscriber_receivers(&mut subscribers);
    println!("background heartbeat tasks started");

    let mut publisher_tasks = Vec::with_capacity(publishers.len());
    for publisher in &publishers {
        publisher_tasks.push(tokio::spawn(run_publisher(
            control_addr,
            publisher.key_index,
            publisher.session_id,
            publisher.socket.clone(),
            config.packets_per_publisher,
            config.payload_bytes,
            config.packet_interval_ms,
        )));
    }

    let mut heartbeat_round = 0usize;
    while heartbeat_round < config.heartbeat_rounds {
        sleep(Duration::from_millis(config.heartbeat_round_interval_ms)).await;
        heartbeat_round += 1;
        println!("heartbeat round {} completed", heartbeat_round);
    }

    while publisher_tasks.iter().any(|task| !task.is_finished()) {
        sleep(Duration::from_millis(config.heartbeat_round_interval_ms)).await;
        heartbeat_round += 1;
        println!(
            "heartbeat round {} completed (auto-extended to cover active publishers)",
            heartbeat_round
        );
    }

    for task in publisher_tasks {
        let result = task
            .await
            .map_err(|err| io::Error::other(format!("publisher task join failed: {err}")))?;
        result.map_err(|err| io::Error::other(format!("publisher task failed: {err}")))?;
    }

    let mut subscriber_results = Vec::with_capacity(subscribers.len());
    for subscriber in &mut subscribers {
        let result = subscriber
            .receive_task
            .take()
            .expect("subscriber receive task should exist")
            .await
            .map_err(|err| io::Error::other(format!("subscriber task join failed: {err}")))?;
        subscriber_results.push(
            result.map_err(|err| io::Error::other(format!("subscriber task failed: {err}")))?,
        );
    }

    let snapshot = query_status_snapshot(status_addr)
        .await
        .map_err(|err| io::Error::other(format!("status snapshot query failed: {err}")))?;
    validate_results(&config, &subscriber_results, &snapshot)?;

    println!("load test succeeded");
    println!(
        "publishers={}, subscribers_per_publisher={}, packets_per_publisher={}, payload_bytes={}",
        config.publishers,
        config.subscribers_per_publisher,
        config.packets_per_publisher,
        config.payload_bytes
    );
    println!(
        "expected_udp_in={}, expected_udp_out={}, elapsed_ms={}",
        config.publishers * config.packets_per_publisher,
        config.publishers * config.subscribers_per_publisher * config.packets_per_publisher,
        start.elapsed().as_millis()
    );
    println!(
        "snapshot: stream_count={}, active_publishers={}, active_subscribers={}, active_udp_publishers={}, active_udp_subscribers={}",
        snapshot.stream_count,
        snapshot.active_publisher_count,
        snapshot.active_subscriber_count,
        snapshot.active_udp_publisher_count,
        snapshot.active_udp_subscriber_count
    );

    stop_heartbeat_tasks(&mut publishers, &mut subscribers)
        .await
        .map_err(|err| io::Error::other(format!("stop heartbeat tasks failed: {err}")))?;
    drop(publishers);
    drop(subscribers);
    sleep(Duration::from_millis(150)).await;

    if let Some(server_task) = server_task {
        server_task.abort();
        let _ = server_task.await;
    }
    Ok(())
}

async fn resolve_socket_addr(host: &str, port: u16) -> io::Result<SocketAddr> {
    let addrs: Vec<SocketAddr> = lookup_host((host, port)).await?.collect();
    if let Some(addr) = addrs.iter().copied().find(SocketAddr::is_ipv4) {
        return Ok(addr);
    }
    addrs.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("no address resolved for {host}:{port}"),
        )
    })
}

async fn connect_publisher(
    control_addr: SocketAddr,
    key_index: usize,
    key: &str,
) -> io::Result<PublisherClient> {
    let (control_reader, control_heartbeat, session_id, udp_port) =
        connect_control(control_addr, ClientRole::Publisher, key)
            .await
            .map_err(|err| io::Error::other(format!("publisher control connect failed: {err}")))?;
    let socket = std::sync::Arc::new(
        bind_udp_socket(
            LOAD_TEST_UDP_PUBLISHER_RECV_BUFFER_BYTES,
            LOAD_TEST_UDP_PUBLISHER_SEND_BUFFER_BYTES,
        )
        .await
        .map_err(|err| io::Error::other(format!("publisher udp socket bind failed: {err}")))?,
    );
    let udp_addr = SocketAddr::new(control_addr.ip(), udp_port);
    udp_register(socket.as_ref(), udp_addr, session_id)
        .await
        .map_err(|err| io::Error::other(format!("publisher udp register failed: {err}")))?;
    let udp_heartbeat = HeartbeatTask::new(tokio::spawn(run_udp_heartbeat(
        socket.clone(),
        control_addr,
        session_id,
    )));

    Ok(PublisherClient {
        key_index,
        _control_reader: control_reader,
        session_id,
        socket,
        control_heartbeat,
        udp_heartbeat,
    })
}

async fn connect_subscriber(
    control_addr: SocketAddr,
    key_index: usize,
    key: &str,
    packets_per_publisher: usize,
    payload_bytes: usize,
) -> io::Result<SubscriberClient> {
    let (control_reader, control_heartbeat, session_id, udp_port) =
        connect_control(control_addr, ClientRole::Subscriber, key)
            .await
            .map_err(|err| io::Error::other(format!("subscriber control connect failed: {err}")))?;
    let socket = std::sync::Arc::new(
        bind_udp_socket(
            LOAD_TEST_UDP_SUBSCRIBER_RECV_BUFFER_BYTES,
            LOAD_TEST_UDP_SUBSCRIBER_SEND_BUFFER_BYTES,
        )
        .await
        .map_err(|err| io::Error::other(format!("subscriber udp socket bind failed: {err}")))?,
    );
    let udp_addr = SocketAddr::new(control_addr.ip(), udp_port);
    udp_register(socket.as_ref(), udp_addr, session_id)
        .await
        .map_err(|err| io::Error::other(format!("subscriber udp register failed: {err}")))?;
    let udp_heartbeat = HeartbeatTask::new(tokio::spawn(run_udp_heartbeat(
        socket.clone(),
        control_addr,
        session_id,
    )));

    Ok(SubscriberClient {
        key_index,
        _control_reader: control_reader,
        session_id,
        socket,
        control_heartbeat,
        udp_heartbeat,
        expected_packets: packets_per_publisher,
        payload_bytes,
        receive_task: None,
    })
}

fn start_subscriber_receivers(subscribers: &mut [SubscriberClient]) {
    for subscriber in subscribers {
        if subscriber.receive_task.is_some() {
            continue;
        }

        let receive_socket = subscriber.socket.clone();
        subscriber.receive_task = Some(tokio::spawn(run_subscriber_receiver(
            subscriber.key_index,
            subscriber.session_id,
            receive_socket,
            subscriber.expected_packets,
            subscriber.payload_bytes,
        )));
    }
}

async fn connect_control(
    control_addr: SocketAddr,
    role: ClientRole,
    key: &str,
) -> io::Result<(
    tokio::net::tcp::OwnedReadHalf,
    HeartbeatTask,
    SessionId,
    u16,
)> {
    let mut stream = TcpStream::connect(control_addr).await?;
    write_json_line(
        &mut stream,
        &HandshakeRequest {
            role,
            key: key.to_owned(),
        },
    )
    .await?;

    let response = read_json_line::<HandshakeResponseOwned, _>(
        &mut stream,
        HANDSHAKE_MAX_BYTES,
        HANDSHAKE_TIMEOUT_MS,
        "handshake response",
    )
    .await?;

    if response.status != "ok" {
        return Err(io::Error::other(format!(
            "handshake failed for key {key}: {}",
            response.message
        )));
    }
    if response.message != "control session established" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unexpected handshake message for key {key}: {}",
                response.message
            ),
        ));
    }
    if response.role != Some(role) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("handshake role mismatch for key {key}"),
        ));
    }
    if response.key.as_deref() != Some(key) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("handshake key mismatch for key {key}"),
        ));
    }
    if response.tcp_heartbeat_interval_ms != Some(TCP_HEARTBEAT_INTERVAL_MS) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected tcp heartbeat interval for key {key}"),
        ));
    }
    if response.udp_session_timeout_ms != Some(UDP_SESSION_TIMEOUT_MS) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected udp session timeout for key {key}"),
        ));
    }
    if response.udp_audio_payload_max_bytes != Some(UDP_AUDIO_PAYLOAD_MAX_BYTES) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected udp payload limit for key {key}"),
        ));
    }

    let session_id = SessionId::from_hex(response.session_id.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing session_id in handshake response",
        )
    })?)
    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;

    let (reader, writer) = stream.into_split();
    Ok((
        reader,
        HeartbeatTask::new(tokio::spawn(run_control_heartbeat(writer))),
        session_id,
        response.udp_port.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "missing udp_port in handshake response",
            )
        })?,
    ))
}

async fn bind_udp_socket(
    recv_buffer_bytes: usize,
    send_buffer_bytes: usize,
) -> io::Result<UdpSocket> {
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_recv_buffer_size(recv_buffer_bytes)?;
    socket.set_send_buffer_size(send_buffer_bytes)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;

    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}

async fn udp_register(
    socket: &UdpSocket,
    udp_addr: SocketAddr,
    session_id: SessionId,
) -> io::Result<()> {
    let packet = encode_udp_register(session_id);
    socket.send_to(&packet, udp_addr).await.map_err(|err| {
        io::Error::other(format!("udp register send failed to {udp_addr}: {err}"))
    })?;

    let mut buffer = [0_u8; UDP_PACKET_MAX_BYTES];
    let (len, _) = timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "udp register ack timed out"))?
        .map_err(|err| io::Error::other(format!("udp register ack receive failed: {err}")))?;

    match parse_udp_packet(&buffer[..len], UDP_AUDIO_PAYLOAD_MAX_BYTES)? {
        nvdaremoteaudio_server::protocol::UdpPacket::RegisterAck {
            session_id: ack_session_id,
        } if ack_session_id == session_id => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "received unexpected udp register ack",
        )),
    }
}

async fn run_control_heartbeat(mut writer: tokio::net::tcp::OwnedWriteHalf) -> io::Result<()> {
    let heartbeat = ControlMessageRequest {
        message_type: ControlMessageType::Heartbeat,
    };

    loop {
        sleep(Duration::from_millis(TCP_HEARTBEAT_INTERVAL_MS)).await;
        write_json_line(&mut writer, &heartbeat).await?;
    }
}

async fn run_udp_heartbeat(
    socket: std::sync::Arc<UdpSocket>,
    control_addr: SocketAddr,
    session_id: SessionId,
) -> io::Result<()> {
    let packet = encode_udp_heartbeat(session_id);

    loop {
        sleep(Duration::from_millis(TCP_HEARTBEAT_INTERVAL_MS)).await;
        socket.send_to(&packet, control_addr).await?;
    }
}

async fn run_publisher(
    control_addr: SocketAddr,
    key_index: usize,
    session_id: SessionId,
    socket: std::sync::Arc<UdpSocket>,
    packets_per_publisher: usize,
    payload_bytes: usize,
    packet_interval_ms: u64,
) -> io::Result<()> {
    for sequence in 0..packets_per_publisher {
        let payload = generate_payload(key_index, sequence as u64, payload_bytes);
        let packet = encode_udp_audio_data(
            session_id,
            sequence as u64,
            generate_timestamp_ms(key_index, sequence as u64),
            &payload,
            UDP_AUDIO_PAYLOAD_MAX_BYTES,
        )?;
        socket.send_to(&packet, control_addr).await?;

        if packet_interval_ms > 0 {
            sleep(Duration::from_millis(packet_interval_ms)).await;
        }
    }

    Ok(())
}

async fn run_subscriber_receiver(
    key_index: usize,
    session_id: SessionId,
    socket: std::sync::Arc<UdpSocket>,
    packets_per_publisher: usize,
    payload_bytes: usize,
) -> io::Result<SubscriberResult> {
    let mut seen = vec![false; packets_per_publisher];
    let mut unique_packets_received = 0;
    let mut duplicate_packets = 0;
    let mut invalid_packets = 0;
    let mut buffer = [0_u8; UDP_PACKET_MAX_BYTES];

    while unique_packets_received < packets_per_publisher {
        let (len, _) = match timeout(
            Duration::from_secs(SUBSCRIBER_RECV_TIMEOUT_SECS),
            socket.recv_from(&mut buffer),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => break,
        };

        match parse_udp_packet(&buffer[..len], UDP_AUDIO_PAYLOAD_MAX_BYTES)? {
            nvdaremoteaudio_server::protocol::UdpPacket::AudioData {
                session_id: packet_session_id,
                sequence,
                timestamp_ms,
                payload,
            } => {
                if packet_session_id != session_id {
                    invalid_packets += 1;
                    continue;
                }

                let Ok(sequence_index) = usize::try_from(sequence) else {
                    invalid_packets += 1;
                    continue;
                };
                if sequence_index >= packets_per_publisher {
                    invalid_packets += 1;
                    continue;
                }

                let expected_payload = generate_payload(key_index, sequence, payload_bytes);
                if payload != expected_payload.as_slice() {
                    invalid_packets += 1;
                    continue;
                }
                if timestamp_ms != generate_timestamp_ms(key_index, sequence) {
                    invalid_packets += 1;
                    continue;
                }

                if seen[sequence_index] {
                    duplicate_packets += 1;
                    continue;
                }

                seen[sequence_index] = true;
                unique_packets_received += 1;
            }
            _ => {
                invalid_packets += 1;
            }
        }
    }

    let missing_packets = seen.iter().filter(|received| !**received).count();
    Ok(SubscriberResult {
        key_index,
        expected_packets: packets_per_publisher,
        unique_packets_received,
        duplicate_packets,
        invalid_packets,
        missing_packets,
    })
}

async fn stop_heartbeat_tasks(
    publishers: &mut [PublisherClient],
    subscribers: &mut [SubscriberClient],
) -> io::Result<()> {
    for publisher in publishers {
        publisher
            .control_heartbeat
            .stop("publisher control")
            .await?;
        publisher.udp_heartbeat.stop("publisher udp").await?;
    }
    for subscriber in subscribers {
        subscriber
            .control_heartbeat
            .stop("subscriber control")
            .await?;
        subscriber.udp_heartbeat.stop("subscriber udp").await?;
    }

    Ok(())
}

async fn query_status_snapshot(status_addr: SocketAddr) -> io::Result<RegistrySnapshot> {
    let mut stream = TcpStream::connect(status_addr).await?;
    write_json_line(
        &mut stream,
        &StatusAccessRequest {
            key: STATUS_ACCESS_KEY.to_owned(),
        },
    )
    .await?;

    read_json_line::<RegistrySnapshot, _>(
        &mut stream,
        STATUS_RESPONSE_MAX_BYTES,
        HANDSHAKE_TIMEOUT_MS,
        "status snapshot",
    )
    .await
}

fn validate_results(
    config: &LoadTestConfig,
    subscriber_results: &[SubscriberResult],
    snapshot: &RegistrySnapshot,
) -> io::Result<()> {
    let expected_subscribers = config.publishers * config.subscribers_per_publisher;
    let expected_udp_in = (config.publishers * config.packets_per_publisher) as u64;
    let expected_udp_out = (config.publishers
        * config.subscribers_per_publisher
        * config.packets_per_publisher) as u64;

    if subscriber_results.len() != expected_subscribers {
        return Err(io::Error::other(format!(
            "subscriber result count mismatch: {} != {}",
            subscriber_results.len(),
            expected_subscribers
        )));
    }

    let mut missing_total = 0;
    let mut invalid_total = 0;
    let mut duplicate_total = 0;
    for result in subscriber_results {
        missing_total += result.missing_packets;
        invalid_total += result.invalid_packets;
        duplicate_total += result.duplicate_packets;

        if result.unique_packets_received != result.expected_packets {
            return Err(io::Error::other(format!(
                "subscriber on key {} received {}/{} packets",
                result.key_index, result.unique_packets_received, result.expected_packets
            )));
        }
    }

    if missing_total != 0 || invalid_total != 0 || duplicate_total != 0 {
        return Err(io::Error::other(format!(
            "subscriber validation failed: missing={}, invalid={}, duplicate={}",
            missing_total, invalid_total, duplicate_total
        )));
    }

    if snapshot.stream_count != config.publishers {
        return Err(io::Error::other(format!(
            "snapshot stream_count mismatch: {} != {}",
            snapshot.stream_count, config.publishers
        )));
    }
    if snapshot.active_publisher_count != config.publishers {
        return Err(io::Error::other(format!(
            "snapshot active_publisher_count mismatch: {} != {}",
            snapshot.active_publisher_count, config.publishers
        )));
    }
    if snapshot.active_subscriber_count != expected_subscribers {
        return Err(io::Error::other(format!(
            "snapshot active_subscriber_count mismatch: {} != {}",
            snapshot.active_subscriber_count, expected_subscribers
        )));
    }
    if snapshot.active_udp_publisher_count != config.publishers {
        return Err(io::Error::other(format!(
            "snapshot active_udp_publisher_count mismatch: {} != {}",
            snapshot.active_udp_publisher_count, config.publishers
        )));
    }
    if snapshot.active_udp_subscriber_count != expected_subscribers {
        return Err(io::Error::other(format!(
            "snapshot active_udp_subscriber_count mismatch: {} != {}",
            snapshot.active_udp_subscriber_count, expected_subscribers
        )));
    }

    let snapshot_udp_in: u64 = snapshot
        .streams
        .iter()
        .map(|stream| stream.udp_audio_packets_in_total)
        .sum();
    let snapshot_udp_out: u64 = snapshot
        .streams
        .iter()
        .map(|stream| stream.udp_audio_packets_out_total)
        .sum();

    if snapshot_udp_in != expected_udp_in {
        return Err(io::Error::other(format!(
            "snapshot udp_audio_packets_in_total mismatch: {} != {}",
            snapshot_udp_in, expected_udp_in
        )));
    }
    if snapshot_udp_out != expected_udp_out {
        return Err(io::Error::other(format!(
            "snapshot udp_audio_packets_out_total mismatch: {} != {}",
            snapshot_udp_out, expected_udp_out
        )));
    }
    if snapshot.invalid_udp_packets_total != 0 {
        return Err(io::Error::other(format!(
            "snapshot invalid_udp_packets_total must be 0, got {}",
            snapshot.invalid_udp_packets_total
        )));
    }
    if snapshot.unknown_udp_session_packets_total != 0 {
        return Err(io::Error::other(format!(
            "snapshot unknown_udp_session_packets_total must be 0, got {}",
            snapshot.unknown_udp_session_packets_total
        )));
    }

    Ok(())
}

fn stream_key(key_index: usize) -> String {
    format!("load_room_{key_index:02}")
}

fn generate_payload(key_index: usize, sequence: u64, payload_bytes: usize) -> Vec<u8> {
    let mut payload = vec![0_u8; payload_bytes];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = ((key_index as u64 * 31 + sequence * 17 + index as u64) & 0xff) as u8;
    }
    payload
}

fn generate_timestamp_ms(key_index: usize, sequence: u64) -> u64 {
    1_760_000_000_000_u64 + (key_index as u64 * 1_000_000) + sequence
}

fn parse_usize(raw: &str, name: &str) -> io::Result<usize> {
    raw.parse::<usize>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be a valid usize"),
        )
    })
}

fn parse_u64(raw: &str, name: &str) -> io::Result<u64> {
    raw.parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be a valid u64"),
        )
    })
}

fn parse_port(raw: &str, name: &str) -> io::Result<u16> {
    let port = raw.parse::<u16>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be a valid u16 port"),
        )
    })?;

    if port == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be greater than 0"),
        ));
    }

    Ok(port)
}
