use std::io;
use std::str;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

pub const MAX_KEY_LEN: usize = 128;
pub const SESSION_ID_BYTES: usize = 16;
pub const UDP_MAGIC: [u8; 4] = *b"RAS1";
pub const UDP_VERSION: u8 = 1;
const UDP_BASE_HEADER_LEN: usize = 4 + 1 + 1 + SESSION_ID_BYTES;
const UDP_AUDIO_META_LEN: usize = 8 + 8;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ClientRole {
    Publisher,
    Subscriber,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HandshakeRequest {
    pub role: ClientRole,
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct HandshakeResponse<'a> {
    pub status: &'a str,
    pub message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<ClientRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub udp_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp_heartbeat_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub udp_session_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub udp_audio_payload_max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct HandshakeResponseOwned {
    pub status: String,
    pub message: String,
    pub role: Option<ClientRole>,
    pub key: Option<String>,
    pub session_id: Option<String>,
    pub udp_port: Option<u16>,
    pub tcp_heartbeat_interval_ms: Option<u64>,
    pub udp_session_timeout_ms: Option<u64>,
    pub udp_audio_payload_max_bytes: Option<usize>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ControlMessageType {
    Heartbeat,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ControlMessageRequest {
    #[serde(rename = "type")]
    pub message_type: ControlMessageType,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StatusAccessRequest {
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct StatusAccessResponse<'a> {
    pub status: &'a str,
    pub message: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct StatusAccessResponseOwned {
    pub status: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SessionId([u8; SESSION_ID_BYTES]);

impl SessionId {
    pub fn random() -> io::Result<Self> {
        let mut bytes = [0_u8; SESSION_ID_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|err| io::Error::other(format!("failed to generate session id: {err}")))?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; SESSION_ID_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; SESSION_ID_BYTES] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(SESSION_ID_BYTES * 2);
        for byte in self.0 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    pub fn from_hex(text: &str) -> Result<Self, &'static str> {
        if text.len() != SESSION_ID_BYTES * 2 {
            return Err("session_id length is invalid");
        }

        let mut bytes = [0_u8; SESSION_ID_BYTES];
        for (index, chunk) in text.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (decode_hex_nibble(chunk[0])? << 4) | decode_hex_nibble(chunk[1])?;
        }

        Ok(Self(bytes))
    }
}

fn decode_hex_nibble(byte: u8) -> Result<u8, &'static str> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("session_id contains invalid hex"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UdpPacketKind {
    Register = 1,
    RegisterAck = 2,
    Heartbeat = 3,
    AudioData = 4,
}

impl UdpPacketKind {
    fn from_u8(value: u8) -> io::Result<Self> {
        match value {
            1 => Ok(Self::Register),
            2 => Ok(Self::RegisterAck),
            3 => Ok(Self::Heartbeat),
            4 => Ok(Self::AudioData),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown udp packet kind: {value}"),
            )),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum UdpPacket<'a> {
    Register {
        session_id: SessionId,
    },
    RegisterAck {
        session_id: SessionId,
    },
    Heartbeat {
        session_id: SessionId,
    },
    AudioData {
        session_id: SessionId,
        sequence: u64,
        timestamp_ms: u64,
        payload: &'a [u8],
    },
}

pub fn validate_key(key: &str) -> Result<(), &'static str> {
    if key.is_empty() {
        return Err("key must not be empty");
    }

    if key.len() > MAX_KEY_LEN {
        return Err("key is too long");
    }

    // NVDA Remote treats its key as an opaque channel/password string and
    // authenticates by exact match. Keep this server compatible by avoiding
    // printable character whitelists, trimming, case folding, or Unicode
    // normalization. Control characters are rejected because they can pollute
    // line-oriented logs and operational tooling without adding useful
    // password compatibility.
    if key.chars().any(char::is_control) {
        return Err("key contains control characters");
    }

    Ok(())
}

pub fn escape_key_for_log(key: &str) -> String {
    key.escape_debug().collect()
}

pub async fn read_json_line<T, R>(
    reader: &mut R,
    max_bytes: usize,
    timeout_ms: u64,
    context: &str,
) -> io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let line = timeout(
        Duration::from_millis(timeout_ms),
        read_line_with_limit(reader, max_bytes),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("{context} timed out")))??;

    serde_json::from_slice::<T>(&line).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {context} json: {err}"),
        )
    })
}

pub async fn write_json_line<T>(writer: &mut (impl AsyncWrite + Unpin), value: &T) -> io::Result<()>
where
    T: Serialize,
{
    let mut payload = serde_json::to_vec(value).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("json encode error: {err}"),
        )
    })?;
    payload.push(b'\n');
    writer.write_all(&payload).await
}

async fn read_line_with_limit<R>(reader: &mut R, max_bytes: usize) -> io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::with_capacity(128);

    loop {
        let mut byte = [0_u8; 1];
        let read = reader.read(&mut byte).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before request finished",
            ));
        }

        if byte[0] == b'\n' {
            break;
        }

        if byte[0] != b'\r' {
            buffer.push(byte[0]);
            if buffer.len() > max_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request is too large",
                ));
            }
        }
    }

    if buffer.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request must not be empty",
        ));
    }

    Ok(buffer)
}

pub fn encode_udp_register(session_id: SessionId) -> Vec<u8> {
    encode_udp_control_packet(UdpPacketKind::Register, session_id)
}

pub fn encode_udp_register_ack(session_id: SessionId) -> Vec<u8> {
    encode_udp_control_packet(UdpPacketKind::RegisterAck, session_id)
}

pub fn encode_udp_heartbeat(session_id: SessionId) -> Vec<u8> {
    encode_udp_control_packet(UdpPacketKind::Heartbeat, session_id)
}

fn encode_udp_control_packet(kind: UdpPacketKind, session_id: SessionId) -> Vec<u8> {
    let mut packet = Vec::with_capacity(UDP_BASE_HEADER_LEN);
    packet.extend_from_slice(&UDP_MAGIC);
    packet.push(UDP_VERSION);
    packet.push(kind as u8);
    packet.extend_from_slice(session_id.as_bytes());
    packet
}

pub fn encode_udp_audio_data(
    session_id: SessionId,
    sequence: u64,
    timestamp_ms: u64,
    payload: &[u8],
    max_audio_payload_bytes: usize,
) -> io::Result<Vec<u8>> {
    if payload.len() > max_audio_payload_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "udp audio payload exceeds limit: {} > {}",
                payload.len(),
                max_audio_payload_bytes
            ),
        ));
    }

    let mut packet = Vec::with_capacity(UDP_BASE_HEADER_LEN + UDP_AUDIO_META_LEN + payload.len());
    packet.extend_from_slice(&UDP_MAGIC);
    packet.push(UDP_VERSION);
    packet.push(UdpPacketKind::AudioData as u8);
    packet.extend_from_slice(session_id.as_bytes());
    packet.extend_from_slice(&sequence.to_be_bytes());
    packet.extend_from_slice(&timestamp_ms.to_be_bytes());
    packet.extend_from_slice(payload);
    Ok(packet)
}

pub fn parse_udp_packet(
    buffer: &[u8],
    max_audio_payload_bytes: usize,
) -> io::Result<UdpPacket<'_>> {
    if buffer.len() < UDP_BASE_HEADER_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "udp packet is too short",
        ));
    }

    if buffer[..4] != UDP_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "udp packet magic is invalid",
        ));
    }

    if buffer[4] != UDP_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported udp version: {}", buffer[4]),
        ));
    }

    let kind = UdpPacketKind::from_u8(buffer[5])?;
    let session_id = SessionId::from_bytes(
        buffer[6..(6 + SESSION_ID_BYTES)]
            .try_into()
            .expect("session_id length is fixed"),
    );

    match kind {
        UdpPacketKind::Register => {
            if buffer.len() != UDP_BASE_HEADER_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "udp register packet size is invalid",
                ));
            }

            Ok(UdpPacket::Register { session_id })
        }
        UdpPacketKind::RegisterAck => {
            if buffer.len() != UDP_BASE_HEADER_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "udp register ack packet size is invalid",
                ));
            }

            Ok(UdpPacket::RegisterAck { session_id })
        }
        UdpPacketKind::Heartbeat => {
            if buffer.len() != UDP_BASE_HEADER_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "udp heartbeat packet size is invalid",
                ));
            }

            Ok(UdpPacket::Heartbeat { session_id })
        }
        UdpPacketKind::AudioData => {
            if buffer.len() < UDP_BASE_HEADER_LEN + UDP_AUDIO_META_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "udp audio packet is too short",
                ));
            }

            let sequence_offset = UDP_BASE_HEADER_LEN;
            let timestamp_offset = sequence_offset + 8;
            let payload_offset = timestamp_offset + 8;
            let payload = &buffer[payload_offset..];
            if payload.len() > max_audio_payload_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "udp audio payload exceeds limit: {} > {}",
                        payload.len(),
                        max_audio_payload_bytes
                    ),
                ));
            }

            let sequence = u64::from_be_bytes(
                buffer[sequence_offset..timestamp_offset]
                    .try_into()
                    .expect("sequence length is fixed"),
            );
            let timestamp_ms = u64::from_be_bytes(
                buffer[timestamp_offset..payload_offset]
                    .try_into()
                    .expect("timestamp length is fixed"),
            );

            Ok(UdpPacket::AudioData {
                session_id,
                sequence,
                timestamp_ms,
                payload,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientRole, ControlMessageRequest, HandshakeRequest, MAX_KEY_LEN, SessionId, UdpPacket,
        encode_udp_audio_data, encode_udp_heartbeat, encode_udp_register, escape_key_for_log,
        parse_udp_packet, validate_key,
    };

    #[test]
    fn accepts_valid_key() {
        assert!(validate_key("123ddd").is_ok());
        assert!(validate_key("stream_01.test-key").is_ok());
        assert!(validate_key("hello world").is_ok());
        assert!(validate_key("bad$key").is_ok());
        assert!(validate_key("NVDA remote 密码 $ 123").is_ok());
    }

    #[test]
    fn rejects_invalid_key() {
        assert!(validate_key("").is_err());
        assert!(validate_key(&"a".repeat(MAX_KEY_LEN + 1)).is_err());
        assert!(validate_key("line\nbreak").is_err());
        assert!(validate_key("tab\tkey").is_err());
        assert!(validate_key("escape\u{1b}key").is_err());
    }

    #[test]
    fn escapes_key_for_log_output() {
        assert_eq!(
            escape_key_for_log("line\nkey\t\u{1b}"),
            "line\\nkey\\t\\u{1b}"
        );
    }

    #[test]
    fn session_id_round_trips_hex() {
        let session_id = SessionId::from_bytes([0x11; 16]);
        let hex = session_id.to_hex();
        assert_eq!(SessionId::from_hex(&hex).unwrap(), session_id);
    }

    #[test]
    fn udp_register_round_trips() {
        let session_id = SessionId::from_bytes([0x22; 16]);
        let packet = encode_udp_register(session_id);
        assert_eq!(
            parse_udp_packet(&packet, 1200).unwrap(),
            UdpPacket::Register { session_id }
        );
    }

    #[test]
    fn udp_heartbeat_round_trips() {
        let session_id = SessionId::from_bytes([0x33; 16]);
        let packet = encode_udp_heartbeat(session_id);
        assert_eq!(
            parse_udp_packet(&packet, 1200).unwrap(),
            UdpPacket::Heartbeat { session_id }
        );
    }

    #[test]
    fn udp_audio_round_trips() {
        let session_id = SessionId::from_bytes([0x44; 16]);
        let packet = encode_udp_audio_data(session_id, 7, 99, b"abc", 1200).unwrap();

        assert_eq!(
            parse_udp_packet(&packet, 1200).unwrap(),
            UdpPacket::AudioData {
                session_id,
                sequence: 7,
                timestamp_ms: 99,
                payload: b"abc",
            }
        );
    }

    #[test]
    fn deserializes_control_messages() {
        let handshake: HandshakeRequest =
            serde_json::from_str(r#"{"role":"publisher","key":"room"}"#).unwrap();
        assert_eq!(handshake.role, ClientRole::Publisher);
        assert_eq!(handshake.key, "room");

        let heartbeat: ControlMessageRequest =
            serde_json::from_str(r#"{"type":"heartbeat"}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&heartbeat.message_type).unwrap(),
            "\"heartbeat\""
        );
    }
}
