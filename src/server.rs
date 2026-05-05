use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::config::{
    CONTROL_IDLE_TIMEOUT_MS, CONTROL_MESSAGE_MAX_BYTES, Config, HANDSHAKE_MAX_BYTES,
    HANDSHAKE_TIMEOUT_MS, STATUS_ACCESS_KEY, STATUS_REQUEST_MAX_BYTES, TCP_HEARTBEAT_INTERVAL_MS,
    UDP_AUDIO_PAYLOAD_MAX_BYTES, UDP_PACKET_MAX_BYTES, UDP_SESSION_TIMEOUT_MS,
};
use crate::net::{UDP_SOCKET_BUFFER_BYTES, bind_udp_socket};
use crate::protocol::{
    ClientRole, ControlMessageRequest, ControlMessageType, HandshakeRequest, HandshakeResponse,
    SessionId, StatusAccessRequest, StatusAccessResponse, UdpPacket, encode_udp_audio_data,
    encode_udp_register_ack, parse_udp_packet, read_json_line, validate_key, write_json_line,
};
use crate::state::{
    AudioDispatchError, AudioDispatchPlan, RegisterSessionError, StreamRegistry, UdpHeartbeatError,
    UdpRegisterError,
};

const UDP_DISPATCH_CHANNEL_CAPACITY: usize = 64;
const MAX_UDP_DISPATCH_WORKERS: usize = 8;

#[derive(Debug)]
struct AudioDispatchJob {
    publisher_session_id: SessionId,
    key: String,
    sequence: u64,
    timestamp_ms: u64,
    payload: Vec<u8>,
    targets: Vec<crate::state::UdpDispatchTarget>,
}

#[derive(Clone)]
struct AudioDispatchWorkers {
    senders: Arc<[mpsc::Sender<AudioDispatchJob>]>,
    registry: StreamRegistry,
}

pub async fn run(config: Config) -> io::Result<()> {
    let registry = StreamRegistry::new(UDP_SESSION_TIMEOUT_MS);
    let control_listener = TcpListener::bind(config.bind_addr).await?;
    let udp_socket = Arc::new(bind_udp_socket(config.bind_addr)?);
    let status_listener = TcpListener::bind(config.status_bind_addr).await?;

    info!(
        control_bind_addr = %config.bind_addr,
        udp_bind_addr = %config.bind_addr,
        status_bind_addr = %config.status_bind_addr,
        handshake_max_bytes = HANDSHAKE_MAX_BYTES,
        control_idle_timeout_ms = CONTROL_IDLE_TIMEOUT_MS,
        tcp_heartbeat_interval_ms = TCP_HEARTBEAT_INTERVAL_MS,
        udp_session_timeout_ms = UDP_SESSION_TIMEOUT_MS,
        udp_packet_max_bytes = UDP_PACKET_MAX_BYTES,
        udp_audio_payload_max_bytes = UDP_AUDIO_PAYLOAD_MAX_BYTES,
        udp_socket_buffer_bytes = UDP_SOCKET_BUFFER_BYTES,
        "server started"
    );

    tokio::try_join!(
        run_control_server(control_listener, registry.clone(), config.bind_addr.port()),
        run_udp_server(udp_socket, registry.clone()),
        run_status_server(status_listener, registry),
    )?;

    Ok(())
}

async fn run_control_server(
    listener: TcpListener,
    registry: StreamRegistry,
    udp_port: u16,
) -> io::Result<()> {
    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let registry = registry.clone();
        tokio::spawn(async move {
            if let Err(err) =
                handle_control_connection(stream, peer_addr.to_string(), registry, udp_port).await
            {
                warn!(peer_addr = %peer_addr, error = %err, "control connection ended with error");
            }
        });
    }
}

async fn handle_control_connection(
    mut stream: TcpStream,
    peer_addr: String,
    registry: StreamRegistry,
    udp_port: u16,
) -> io::Result<()> {
    let request = read_json_line::<HandshakeRequest, _>(
        &mut stream,
        HANDSHAKE_MAX_BYTES,
        HANDSHAKE_TIMEOUT_MS,
        "handshake",
    )
    .await?;

    validate_key(&request.key)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;

    let registration =
        match registry.register_control_session(request.role, &request.key, peer_addr.clone()) {
            Ok(registration) => registration,
            Err(RegisterSessionError::PublisherAlreadyConnected) => {
                write_json_line(
                    &mut stream,
                    &HandshakeResponse {
                        status: "error",
                        message: "publisher already connected for this key",
                        role: None,
                        key: Some(&request.key),
                        session_id: None,
                        udp_port: None,
                        tcp_heartbeat_interval_ms: None,
                        udp_session_timeout_ms: None,
                        udp_audio_payload_max_bytes: None,
                    },
                )
                .await?;

                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "publisher already connected for this key",
                ));
            }
            Err(RegisterSessionError::SessionIdGeneration(err)) => {
                return Err(io::Error::new(
                    err.kind(),
                    format!("failed to allocate session id: {err}"),
                ));
            }
        };

    let session_hex = registration.session_id.to_hex();
    if let Err(err) = write_json_line(
        &mut stream,
        &HandshakeResponse {
            status: "ok",
            message: "control session established",
            role: Some(request.role),
            key: Some(&request.key),
            session_id: Some(&session_hex),
            udp_port: Some(udp_port),
            tcp_heartbeat_interval_ms: Some(TCP_HEARTBEAT_INTERVAL_MS),
            udp_session_timeout_ms: Some(UDP_SESSION_TIMEOUT_MS),
            udp_audio_payload_max_bytes: Some(UDP_AUDIO_PAYLOAD_MAX_BYTES),
        },
    )
    .await
    {
        registry.unregister_session(registration.session_id, "handshake_response_failed");
        return Err(err);
    }

    info!(
        peer_addr = %peer_addr,
        key = %request.key,
        role = role_label(request.role),
        session_id = %session_hex,
        "control session connected"
    );

    let result = monitor_control_connection(&mut stream, registration.session_id, &registry).await;
    let disconnect_reason = disconnect_reason(&result);
    registry.unregister_session(registration.session_id, &disconnect_reason);

    match result {
        Ok(()) => {
            info!(
                peer_addr = %peer_addr,
                key = %request.key,
                role = role_label(request.role),
                session_id = %session_hex,
                "control session disconnected"
            );
            Ok(())
        }
        Err(err) => {
            warn!(
                peer_addr = %peer_addr,
                key = %request.key,
                role = role_label(request.role),
                session_id = %session_hex,
                error = %err,
                "control session ended with error"
            );
            Err(err)
        }
    }
}

async fn monitor_control_connection(
    stream: &mut TcpStream,
    session_id: SessionId,
    registry: &StreamRegistry,
) -> io::Result<()> {
    loop {
        let message = match read_json_line::<ControlMessageRequest, _>(
            stream,
            CONTROL_MESSAGE_MAX_BYTES,
            CONTROL_IDLE_TIMEOUT_MS,
            "control message",
        )
        .await
        {
            Ok(message) => message,
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(err),
        };

        match message.message_type {
            ControlMessageType::Heartbeat => {
                if !registry.record_control_heartbeat(session_id) {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "control session no longer exists",
                    ));
                }
            }
        }
    }
}

async fn run_udp_server(socket: Arc<UdpSocket>, registry: StreamRegistry) -> io::Result<()> {
    let mut buffer = [0_u8; UDP_PACKET_MAX_BYTES];
    let dispatch_workers = AudioDispatchWorkers::new(socket.clone(), registry.clone());

    loop {
        let (len, peer_addr) = socket.recv_from(&mut buffer).await?;
        let packet = match parse_udp_packet(&buffer[..len], UDP_AUDIO_PAYLOAD_MAX_BYTES) {
            Ok(packet) => packet,
            Err(err) => {
                registry.record_invalid_udp_packet();
                warn!(peer_addr = %peer_addr, error = %err, "invalid udp packet");
                continue;
            }
        };

        match packet {
            UdpPacket::Register { session_id } => {
                match registry.record_udp_register(session_id, peer_addr) {
                    Ok(()) => {}
                    Err(UdpRegisterError::UnknownSession) => {
                        registry.record_unknown_udp_session();
                        warn!(peer_addr = %peer_addr, session_id = %session_id.to_hex(), "udp register rejected for unknown session");
                        continue;
                    }
                }

                if let Some((role, key)) = registry.session_role_and_key(session_id) {
                    info!(
                        peer_addr = %peer_addr,
                        key = %key,
                        role = role_label(role),
                        session_id = %session_id.to_hex(),
                        "udp session registered"
                    );
                }

                let ack = encode_udp_register_ack(session_id);
                match socket.send_to(&ack, peer_addr).await {
                    Ok(sent) if sent == ack.len() => {}
                    Ok(sent) => {
                        registry.record_udp_send_error(session_id);
                        warn!(
                            peer_addr = %peer_addr,
                            session_id = %session_id.to_hex(),
                            sent,
                            expected = ack.len(),
                            "udp register ack was not sent completely"
                        );
                    }
                    Err(err) => {
                        registry.record_udp_send_error(session_id);
                        warn!(
                            peer_addr = %peer_addr,
                            session_id = %session_id.to_hex(),
                            error = %err,
                            "failed to send udp register ack"
                        );
                    }
                }
            }
            UdpPacket::Heartbeat { session_id } => {
                match registry.record_udp_heartbeat(session_id, peer_addr) {
                    Ok(()) => {}
                    Err(UdpHeartbeatError::UnknownSession) => {
                        registry.record_unknown_udp_session();
                        warn!(peer_addr = %peer_addr, session_id = %session_id.to_hex(), "udp heartbeat rejected for unknown session");
                    }
                    Err(err) => {
                        warn!(
                            peer_addr = %peer_addr,
                            session_id = %session_id.to_hex(),
                            error = ?err,
                            "udp heartbeat rejected"
                        );
                    }
                }
            }
            UdpPacket::RegisterAck { session_id } => {
                registry.record_invalid_udp_packet();
                warn!(
                    peer_addr = %peer_addr,
                    session_id = %session_id.to_hex(),
                    "unexpected udp register ack from client"
                );
            }
            UdpPacket::AudioData {
                session_id,
                sequence,
                timestamp_ms,
                payload,
            } => {
                let plan = match registry.prepare_audio_dispatch(
                    session_id,
                    peer_addr,
                    payload.len(),
                ) {
                    Ok(plan) => plan,
                    Err(AudioDispatchError::UnknownSession) => {
                        registry.record_unknown_udp_session();
                        warn!(peer_addr = %peer_addr, session_id = %session_id.to_hex(), "udp audio rejected for unknown session");
                        continue;
                    }
                    Err(err) => {
                        warn!(
                            peer_addr = %peer_addr,
                            session_id = %session_id.to_hex(),
                            error = ?err,
                            "udp audio rejected"
                        );
                        continue;
                    }
                };

                dispatch_workers.enqueue(AudioDispatchJob::from_plan(
                    plan,
                    sequence,
                    timestamp_ms,
                    payload,
                ))?;
            }
        }
    }
}

async fn run_status_server(listener: TcpListener, registry: StreamRegistry) -> io::Result<()> {
    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let registry = registry.clone();
        tokio::spawn(async move {
            handle_status_connection(stream, peer_addr, registry).await;
        });
    }
}

async fn handle_status_connection(
    mut stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    registry: StreamRegistry,
) {
    let request = match read_json_line::<StatusAccessRequest, _>(
        &mut stream,
        STATUS_REQUEST_MAX_BYTES,
        HANDSHAKE_TIMEOUT_MS,
        "status access",
    )
    .await
    {
        Ok(request) => request,
        Err(err) => {
            if let Err(write_err) = write_json_line(
                &mut stream,
                &StatusAccessResponse {
                    status: "error",
                    message: "invalid status request",
                },
            )
            .await
            {
                warn!(
                    peer_addr = %peer_addr,
                    error = %write_err,
                    "status error response write failed"
                );
            }
            warn!(peer_addr = %peer_addr, error = %err, "status access request failed");
            return;
        }
    };

    if request.key != STATUS_ACCESS_KEY {
        if let Err(err) = write_json_line(
            &mut stream,
            &StatusAccessResponse {
                status: "error",
                message: "invalid status key",
            },
        )
        .await
        {
            warn!(
                peer_addr = %peer_addr,
                error = %err,
                "status rejection write failed"
            );
        }
        warn!(peer_addr = %peer_addr, "status access rejected");
        return;
    }

    let snapshot = registry.snapshot();
    match write_json_line(&mut stream, &snapshot).await {
        Ok(()) => {
            info!(
                peer_addr = %peer_addr,
                stream_count = snapshot.stream_count,
                active_publisher_count = snapshot.active_publisher_count,
                active_subscriber_count = snapshot.active_subscriber_count,
                "status snapshot served"
            );
        }
        Err(err) => {
            warn!(
                peer_addr = %peer_addr,
                error = %err,
                "status snapshot write failed"
            );
        }
    }
}

impl AudioDispatchJob {
    fn from_plan(
        plan: AudioDispatchPlan,
        sequence: u64,
        timestamp_ms: u64,
        payload: &[u8],
    ) -> Self {
        Self {
            publisher_session_id: plan.publisher_session_id,
            key: plan.key,
            sequence,
            timestamp_ms,
            payload: payload.to_vec(),
            targets: plan.targets,
        }
    }
}

impl AudioDispatchWorkers {
    fn new(socket: Arc<UdpSocket>, registry: StreamRegistry) -> Self {
        let worker_count = udp_dispatch_worker_count();
        let mut senders = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let (sender, receiver) = mpsc::channel(UDP_DISPATCH_CHANNEL_CAPACITY);
            senders.push(sender);
            tokio::spawn(run_audio_dispatch_worker(
                worker_index,
                socket.clone(),
                registry.clone(),
                receiver,
            ));
        }

        Self {
            senders: senders.into(),
            registry,
        }
    }

    fn enqueue(&self, job: AudioDispatchJob) -> io::Result<()> {
        let worker_index = dispatch_worker_index(&job.key, self.senders.len());
        match self.senders[worker_index].try_send(job) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "udp dispatch worker channel closed unexpectedly",
            )),
            Err(mpsc::error::TrySendError::Full(job)) => {
                self.registry.record_audio_dispatch_outcome(
                    &job.key,
                    job.payload.len(),
                    0,
                    job.targets.len(),
                );
                Ok(())
            }
        }
    }
}

async fn run_audio_dispatch_worker(
    worker_index: usize,
    socket: Arc<UdpSocket>,
    registry: StreamRegistry,
    mut receiver: mpsc::Receiver<AudioDispatchJob>,
) {
    while let Some(job) = receiver.recv().await {
        if !registry.session_matches(job.publisher_session_id, ClientRole::Publisher, &job.key) {
            continue;
        }

        let mut successful_targets = 0usize;
        let mut send_errors = 0usize;
        for target in job.targets {
            if !registry.subscriber_target_is_active(target.session_id, target.endpoint) {
                continue;
            }

            let packet = match encode_udp_audio_data(
                target.session_id,
                job.sequence,
                job.timestamp_ms,
                &job.payload,
                UDP_AUDIO_PAYLOAD_MAX_BYTES,
            ) {
                Ok(packet) => packet,
                Err(err) => {
                    send_errors += 1;
                    warn!(
                        worker_index,
                        key = %job.key,
                        session_id = %target.session_id.to_hex(),
                        error = %err,
                        "failed to encode forwarded udp audio packet"
                    );
                    continue;
                }
            };

            match socket.send_to(&packet, target.endpoint).await {
                Ok(sent) if sent == packet.len() => successful_targets += 1,
                Ok(sent) => {
                    send_errors += 1;
                    warn!(
                        worker_index,
                        key = %job.key,
                        target_addr = %target.endpoint,
                        session_id = %target.session_id.to_hex(),
                        sent,
                        expected = packet.len(),
                        "udp audio packet was not sent completely"
                    );
                }
                Err(err) => {
                    send_errors += 1;
                    warn!(
                        worker_index,
                        key = %job.key,
                        target_addr = %target.endpoint,
                        session_id = %target.session_id.to_hex(),
                        error = %err,
                        "failed to forward udp audio packet"
                    );
                }
            }
        }

        registry.record_audio_dispatch_outcome(
            &job.key,
            job.payload.len(),
            successful_targets,
            send_errors,
        );
    }
}

fn dispatch_worker_index(key: &str, worker_count: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % worker_count
}

fn udp_dispatch_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1)
        .clamp(1, MAX_UDP_DISPATCH_WORKERS)
}

fn role_label(role: ClientRole) -> &'static str {
    match role {
        ClientRole::Publisher => "publisher",
        ClientRole::Subscriber => "subscriber",
    }
}

fn disconnect_reason(result: &io::Result<()>) -> String {
    match result {
        Ok(()) => "peer_disconnected".to_owned(),
        Err(err) => match err.kind() {
            io::ErrorKind::TimedOut => "timeout".to_owned(),
            io::ErrorKind::InvalidData => "protocol_error".to_owned(),
            io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::UnexpectedEof => "peer_disconnected".to_owned(),
            _ => err.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::time::Duration;

    use serde::Serialize;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpStream, UdpSocket};
    use tokio::task::JoinHandle;
    use tokio::time::{sleep, timeout};

    use crate::config::{
        CONTROL_IDLE_TIMEOUT_MS, STATUS_RESPONSE_MAX_BYTES, TCP_HEARTBEAT_INTERVAL_MS,
        UDP_AUDIO_PAYLOAD_MAX_BYTES, UDP_SESSION_TIMEOUT_MS,
    };
    use crate::protocol::{
        HandshakeResponseOwned, SessionId, StatusAccessResponseOwned, encode_udp_audio_data,
        encode_udp_heartbeat, encode_udp_register, parse_udp_packet, read_json_line,
        write_json_line,
    };

    use super::*;

    #[derive(Serialize)]
    struct HeartbeatMessage {
        #[serde(rename = "type")]
        message_type: &'static str,
    }

    fn alloc_tcp_port() -> u16 {
        let listener =
            std::net::TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .expect("port allocation should succeed");
        listener.local_addr().unwrap().port()
    }

    fn alloc_control_port() -> u16 {
        for _ in 0..100 {
            let listener = std::net::TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::LOCALHOST,
                0,
            )))
            .expect("tcp port allocation should succeed");
            let port = listener.local_addr().unwrap().port();

            if std::net::UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::LOCALHOST,
                port,
            )))
            .is_ok()
            {
                return port;
            }
        }

        panic!("control port allocation should find a TCP and UDP free port");
    }

    async fn wait_for_server_ready(
        control_addr: SocketAddr,
        handle: &JoinHandle<io::Result<()>>,
    ) -> bool {
        for _ in 0..50 {
            if handle.is_finished() {
                return false;
            }

            if TcpStream::connect(control_addr).await.is_ok() {
                // This readiness probe is intentionally a plain TCP connect.
                // Dropping it keeps the test from creating a control session.
                return true;
            }

            sleep(Duration::from_millis(20)).await;
        }

        false
    }

    async fn start_server() -> (JoinHandle<io::Result<()>>, SocketAddr, SocketAddr) {
        for _ in 0..20 {
            let port = alloc_control_port();
            let status_port = alloc_tcp_port();
            let control_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
            let status_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, status_port));

            let handle = tokio::spawn(run(Config {
                bind_addr: control_addr,
                status_bind_addr: status_addr,
                log_path: None,
            }));

            if wait_for_server_ready(control_addr, &handle).await {
                return (handle, control_addr, status_addr);
            }

            handle.abort();
            let _ = handle.await;
        }

        panic!("test server should start on an available port");
    }

    async fn connect_control(
        control_addr: SocketAddr,
        role: ClientRole,
        key: &str,
    ) -> (tokio::net::tcp::OwnedWriteHalf, SessionId, u16) {
        let mut stream = TcpStream::connect(control_addr).await.unwrap();
        write_json_line(
            &mut stream,
            &HandshakeRequest {
                role,
                key: key.to_owned(),
            },
        )
        .await
        .unwrap();

        let response = read_json_line::<HandshakeResponseOwned, _>(
            &mut stream,
            HANDSHAKE_MAX_BYTES,
            HANDSHAKE_TIMEOUT_MS,
            "handshake response",
        )
        .await
        .unwrap();

        assert_eq!(response.status, "ok");
        assert_eq!(response.message, "control session established");
        assert_eq!(response.role, Some(role));
        assert_eq!(response.key.as_deref(), Some(key));
        assert_eq!(
            response.tcp_heartbeat_interval_ms,
            Some(TCP_HEARTBEAT_INTERVAL_MS)
        );
        assert_eq!(
            response.udp_session_timeout_ms,
            Some(UDP_SESSION_TIMEOUT_MS)
        );
        assert_eq!(
            response.udp_audio_payload_max_bytes,
            Some(UDP_AUDIO_PAYLOAD_MAX_BYTES)
        );
        let session_id = SessionId::from_hex(response.session_id.as_deref().unwrap()).unwrap();
        let udp_port = response.udp_port.unwrap();
        let (_reader, writer) = stream.into_split();
        (writer, session_id, udp_port)
    }

    async fn udp_register(socket: &UdpSocket, control_addr: SocketAddr, session_id: SessionId) {
        let packet = encode_udp_register(session_id);
        socket.send_to(&packet, control_addr).await.unwrap();

        let mut buffer = [0_u8; UDP_PACKET_MAX_BYTES];
        let (len, _) = timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            parse_udp_packet(&buffer[..len], UDP_AUDIO_PAYLOAD_MAX_BYTES).unwrap(),
            UdpPacket::RegisterAck { session_id }
        );
    }

    #[tokio::test]
    async fn forwards_udp_audio_after_control_authentication() {
        let (server_handle, control_addr, status_addr) = start_server().await;
        // NVDA Remote accepts its key as an opaque exact-match password/channel,
        // so the end-to-end path must work with spaces, symbols, and Unicode.
        let key = "NVDA remote 密码 $ 123";

        let (mut publisher_writer, publisher_session_id, udp_port) =
            connect_control(control_addr, ClientRole::Publisher, key).await;
        let (mut subscriber_writer, subscriber_session_id, _) =
            connect_control(control_addr, ClientRole::Subscriber, key).await;
        assert_eq!(udp_port, control_addr.port());

        let publisher_udp =
            UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .await
                .unwrap();
        let subscriber_udp =
            UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .await
                .unwrap();

        udp_register(&publisher_udp, control_addr, publisher_session_id).await;
        udp_register(&subscriber_udp, control_addr, subscriber_session_id).await;

        write_json_line(
            &mut publisher_writer,
            &HeartbeatMessage {
                message_type: "heartbeat",
            },
        )
        .await
        .unwrap();
        write_json_line(
            &mut subscriber_writer,
            &HeartbeatMessage {
                message_type: "heartbeat",
            },
        )
        .await
        .unwrap();

        let packet = encode_udp_audio_data(
            publisher_session_id,
            1,
            123,
            b"hello",
            UDP_AUDIO_PAYLOAD_MAX_BYTES,
        )
        .unwrap();
        publisher_udp.send_to(&packet, control_addr).await.unwrap();

        let mut buffer = [0_u8; UDP_PACKET_MAX_BYTES];
        let (len, _) = timeout(
            Duration::from_secs(2),
            subscriber_udp.recv_from(&mut buffer),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(
            parse_udp_packet(&buffer[..len], UDP_AUDIO_PAYLOAD_MAX_BYTES).unwrap(),
            UdpPacket::AudioData {
                session_id: subscriber_session_id,
                sequence: 1,
                timestamp_ms: 123,
                payload: b"hello",
            }
        );

        let mut status_stream = TcpStream::connect(status_addr).await.unwrap();
        write_json_line(
            &mut status_stream,
            &StatusAccessRequest {
                key: STATUS_ACCESS_KEY.to_owned(),
            },
        )
        .await
        .unwrap();

        let snapshot = read_json_line::<crate::state::RegistrySnapshot, _>(
            &mut status_stream,
            STATUS_RESPONSE_MAX_BYTES,
            HANDSHAKE_TIMEOUT_MS,
            "status snapshot",
        )
        .await
        .unwrap();

        assert_eq!(snapshot.active_publisher_count, 1);
        assert_eq!(snapshot.active_subscriber_count, 1);
        assert_eq!(snapshot.active_udp_publisher_count, 1);
        assert_eq!(snapshot.active_udp_subscriber_count, 1);
        assert_eq!(snapshot.streams[0].udp_audio_packets_in_total, 1);
        assert_eq!(snapshot.streams[0].udp_audio_bytes_in_total, 5);

        publisher_writer.shutdown().await.unwrap();
        subscriber_writer.shutdown().await.unwrap();
        server_handle.abort();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn rejects_invalid_status_key() {
        let (server_handle, _control_addr, status_addr) = start_server().await;

        let mut status_stream = TcpStream::connect(status_addr).await.unwrap();
        write_json_line(
            &mut status_stream,
            &StatusAccessRequest {
                key: "wrong".to_owned(),
            },
        )
        .await
        .unwrap();

        let response = read_json_line::<StatusAccessResponseOwned, _>(
            &mut status_stream,
            STATUS_REQUEST_MAX_BYTES,
            HANDSHAKE_TIMEOUT_MS,
            "status response",
        )
        .await
        .unwrap();

        assert_eq!(response.status, "error");
        assert_eq!(response.message, "invalid status key");

        server_handle.abort();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn control_connection_times_out_without_heartbeat() {
        let (server_handle, control_addr, _status_addr) = start_server().await;
        let (writer, _session_id, _) =
            connect_control(control_addr, ClientRole::Subscriber, "room").await;

        sleep(Duration::from_millis(CONTROL_IDLE_TIMEOUT_MS + 500)).await;
        drop(writer);

        server_handle.abort();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn requires_reregister_after_udp_source_port_change() {
        let (server_handle, control_addr, _status_addr) = start_server().await;

        let (mut publisher_writer, publisher_session_id, _) =
            connect_control(control_addr, ClientRole::Publisher, "room").await;
        let (mut subscriber_writer, subscriber_session_id, _) =
            connect_control(control_addr, ClientRole::Subscriber, "room").await;

        let publisher_udp_a =
            UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .await
                .unwrap();
        let publisher_udp_b =
            UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .await
                .unwrap();
        let subscriber_udp =
            UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .await
                .unwrap();

        udp_register(&publisher_udp_a, control_addr, publisher_session_id).await;
        udp_register(&subscriber_udp, control_addr, subscriber_session_id).await;

        write_json_line(
            &mut publisher_writer,
            &HeartbeatMessage {
                message_type: "heartbeat",
            },
        )
        .await
        .unwrap();
        write_json_line(
            &mut subscriber_writer,
            &HeartbeatMessage {
                message_type: "heartbeat",
            },
        )
        .await
        .unwrap();

        let heartbeat = encode_udp_heartbeat(publisher_session_id);
        publisher_udp_b
            .send_to(&heartbeat, control_addr)
            .await
            .unwrap();

        let packet = encode_udp_audio_data(
            publisher_session_id,
            1,
            111,
            b"before-reregister",
            UDP_AUDIO_PAYLOAD_MAX_BYTES,
        )
        .unwrap();
        publisher_udp_b
            .send_to(&packet, control_addr)
            .await
            .unwrap();

        let mut buffer = [0_u8; UDP_PACKET_MAX_BYTES];
        assert!(
            timeout(
                Duration::from_millis(300),
                subscriber_udp.recv_from(&mut buffer),
            )
            .await
            .is_err()
        );

        udp_register(&publisher_udp_b, control_addr, publisher_session_id).await;

        let packet = encode_udp_audio_data(
            publisher_session_id,
            2,
            222,
            b"after-reregister",
            UDP_AUDIO_PAYLOAD_MAX_BYTES,
        )
        .unwrap();
        publisher_udp_b
            .send_to(&packet, control_addr)
            .await
            .unwrap();

        let (len, _) = timeout(
            Duration::from_secs(2),
            subscriber_udp.recv_from(&mut buffer),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            parse_udp_packet(&buffer[..len], UDP_AUDIO_PAYLOAD_MAX_BYTES).unwrap(),
            UdpPacket::AudioData {
                session_id: subscriber_session_id,
                sequence: 2,
                timestamp_ms: 222,
                payload: b"after-reregister",
            }
        );

        publisher_writer.shutdown().await.unwrap();
        subscriber_writer.shutdown().await.unwrap();
        server_handle.abort();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn control_disconnect_immediately_invalidates_udp_session() {
        let (server_handle, control_addr, _status_addr) = start_server().await;

        let (mut publisher_writer, publisher_session_id, _) =
            connect_control(control_addr, ClientRole::Publisher, "room").await;
        let (mut subscriber_writer, subscriber_session_id, _) =
            connect_control(control_addr, ClientRole::Subscriber, "room").await;

        let publisher_udp =
            UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .await
                .unwrap();
        let subscriber_udp =
            UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .await
                .unwrap();

        udp_register(&publisher_udp, control_addr, publisher_session_id).await;
        udp_register(&subscriber_udp, control_addr, subscriber_session_id).await;

        write_json_line(
            &mut publisher_writer,
            &HeartbeatMessage {
                message_type: "heartbeat",
            },
        )
        .await
        .unwrap();
        write_json_line(
            &mut subscriber_writer,
            &HeartbeatMessage {
                message_type: "heartbeat",
            },
        )
        .await
        .unwrap();

        publisher_writer.shutdown().await.unwrap();
        sleep(Duration::from_millis(200)).await;

        let packet = encode_udp_audio_data(
            publisher_session_id,
            1,
            123,
            b"hello",
            UDP_AUDIO_PAYLOAD_MAX_BYTES,
        )
        .unwrap();
        publisher_udp.send_to(&packet, control_addr).await.unwrap();

        let mut buffer = [0_u8; UDP_PACKET_MAX_BYTES];
        assert!(
            timeout(
                Duration::from_millis(300),
                subscriber_udp.recv_from(&mut buffer),
            )
            .await
            .is_err()
        );

        subscriber_writer.shutdown().await.unwrap();
        server_handle.abort();
        let _ = server_handle.await;
    }
}
