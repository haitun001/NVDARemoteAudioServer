use std::env;
use std::ffi::OsString;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

const DEFAULT_PORT: u16 = 6838;
const DEFAULT_STATUS_PORT: u16 = 6839;
pub const STATUS_ACCESS_KEY: &str = "audiostatus";
pub const HANDSHAKE_MAX_BYTES: usize = 4096;
pub const CONTROL_MESSAGE_MAX_BYTES: usize = 1024;
pub const STATUS_REQUEST_MAX_BYTES: usize = 1024;
pub const STATUS_RESPONSE_MAX_BYTES: usize = 16 * 1024 * 1024;
pub const HANDSHAKE_TIMEOUT_MS: u64 = 5000;
pub const CONTROL_IDLE_TIMEOUT_MS: u64 = 15000;
pub const TCP_HEARTBEAT_INTERVAL_MS: u64 = 5000;
pub const UDP_SESSION_TIMEOUT_MS: u64 = 15000;
pub const UDP_PACKET_MAX_BYTES: usize = 1400;
pub const UDP_AUDIO_PAYLOAD_MAX_BYTES: usize = 1200;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub status_bind_addr: SocketAddr,
    pub log_path: Option<PathBuf>,
}

impl Config {
    pub fn from_args() -> io::Result<Self> {
        let mut port = DEFAULT_PORT;
        let mut status_port = DEFAULT_STATUS_PORT;
        let mut log_path = None;

        for arg in env::args_os().skip(1) {
            if let Some(value) = value_after_prefix(&arg, "--port=") {
                port = parse_port(&value, "--port")?;
                continue;
            }

            if let Some(value) = value_after_prefix(&arg, "--sport=") {
                status_port = parse_port(&value, "--sport")?;
                continue;
            }

            if let Some(value) = value_after_prefix(&arg, "--log=") {
                if value.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--log requires a non-empty file path",
                    ));
                }

                log_path = Some(PathBuf::from(value));
                continue;
            }

            if arg == "--help" || arg == "-h" {
                return Err(io::Error::other(usage_text()));
            }

            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown argument: {:?}\n{}", arg, usage_text()),
            ));
        }

        let bind_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

        Ok(Self {
            bind_addr: SocketAddr::new(bind_ip, port),
            status_bind_addr: SocketAddr::new(bind_ip, status_port),
            log_path,
        })
    }
}

fn parse_port(raw: &str, name: &str) -> io::Result<u16> {
    let port = raw.parse::<u16>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be a valid u16 port number"),
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

fn value_after_prefix(arg: &OsString, prefix: &str) -> Option<String> {
    let text = arg.to_str()?;
    text.strip_prefix(prefix).map(ToOwned::to_owned)
}

fn usage_text() -> &'static str {
    "usage: NVDARemoteAudioServer [--port=6838] [--sport=6839] [--log=/path/to/server.log]"
}
