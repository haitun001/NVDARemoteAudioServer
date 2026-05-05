use std::collections::{HashMap, HashSet};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::protocol::{ClientRole, SessionId};

#[derive(Clone)]
pub struct StreamRegistry {
    inner: Arc<Mutex<RegistryInner>>,
    udp_session_timeout_ms: u64,
}

struct RegistryInner {
    streams: HashMap<String, StreamEntry>,
    sessions: HashMap<SessionId, SessionEntry>,
    invalid_udp_packets_total: u64,
    unknown_udp_session_packets_total: u64,
}

struct SessionEntry {
    role: ClientRole,
    key: String,
    tcp_peer_addr: String,
    udp_endpoint: Option<SocketAddr>,
    udp_last_seen_unix_ms: Option<u64>,
}

struct StreamEntry {
    publisher_session: Option<SessionId>,
    subscribers: HashSet<SessionId>,
    publisher_connections_total: u64,
    subscriber_connections_total: u64,
    publisher_tcp_heartbeats_total: u64,
    subscriber_tcp_heartbeats_total: u64,
    publisher_udp_registers_total: u64,
    subscriber_udp_registers_total: u64,
    publisher_udp_heartbeats_total: u64,
    subscriber_udp_heartbeats_total: u64,
    udp_audio_packets_in_total: u64,
    udp_audio_bytes_in_total: u64,
    udp_audio_packets_out_total: u64,
    udp_audio_bytes_out_total: u64,
    udp_send_errors_total: u64,
    last_activity_unix_ms: u64,
    last_publisher_disconnect_reason: Option<String>,
    last_subscriber_disconnect_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionRegistration {
    pub session_id: SessionId,
}

#[derive(Debug)]
pub enum RegisterSessionError {
    PublisherAlreadyConnected,
    SessionIdGeneration(io::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UdpDispatchTarget {
    pub session_id: SessionId,
    pub endpoint: SocketAddr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioDispatchPlan {
    pub publisher_session_id: SessionId,
    pub key: String,
    pub targets: Vec<UdpDispatchTarget>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum AudioDispatchError {
    UnknownSession,
    WrongRole,
    UdpNotRegistered,
    UdpEndpointMismatch,
    UdpEndpointExpired,
}

#[derive(Debug, Eq, PartialEq)]
pub enum UdpRegisterError {
    UnknownSession,
}

#[derive(Debug, Eq, PartialEq)]
pub enum UdpHeartbeatError {
    UnknownSession,
    UdpNotRegistered,
    UdpEndpointMismatch,
    UdpEndpointExpired,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RegistrySnapshot {
    pub generated_at_unix_ms: u64,
    pub stream_count: usize,
    pub active_publisher_count: usize,
    pub active_subscriber_count: usize,
    pub active_udp_publisher_count: usize,
    pub active_udp_subscriber_count: usize,
    pub invalid_udp_packets_total: u64,
    pub unknown_udp_session_packets_total: u64,
    pub streams: Vec<StreamSnapshot>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StreamSnapshot {
    pub key: String,
    pub publisher_control_connected: bool,
    pub publisher_udp_registered: bool,
    pub subscriber_count: usize,
    pub subscriber_udp_registered_count: usize,
    pub publisher_connections_total: u64,
    pub subscriber_connections_total: u64,
    pub publisher_tcp_heartbeats_total: u64,
    pub subscriber_tcp_heartbeats_total: u64,
    pub publisher_udp_registers_total: u64,
    pub subscriber_udp_registers_total: u64,
    pub publisher_udp_heartbeats_total: u64,
    pub subscriber_udp_heartbeats_total: u64,
    pub udp_audio_packets_in_total: u64,
    pub udp_audio_bytes_in_total: u64,
    pub udp_audio_packets_out_total: u64,
    pub udp_audio_bytes_out_total: u64,
    pub udp_send_errors_total: u64,
    pub last_activity_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_publisher_disconnect_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_subscriber_disconnect_reason: Option<String>,
}

impl StreamRegistry {
    pub fn new(udp_session_timeout_ms: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                streams: HashMap::new(),
                sessions: HashMap::new(),
                invalid_udp_packets_total: 0,
                unknown_udp_session_packets_total: 0,
            })),
            udp_session_timeout_ms,
        }
    }

    pub fn register_control_session(
        &self,
        role: ClientRole,
        key: &str,
        tcp_peer_addr: String,
    ) -> Result<SessionRegistration, RegisterSessionError> {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        if role == ClientRole::Publisher
            && guard
                .streams
                .get(key)
                .is_some_and(|stream| stream.publisher_session.is_some())
        {
            return Err(RegisterSessionError::PublisherAlreadyConnected);
        }

        let session_id = loop {
            let candidate =
                SessionId::random().map_err(RegisterSessionError::SessionIdGeneration)?;
            if !guard.sessions.contains_key(&candidate) {
                break candidate;
            }
        };

        let now = unix_now_ms();
        guard.sessions.insert(
            session_id,
            SessionEntry {
                role,
                key: key.to_owned(),
                tcp_peer_addr,
                udp_endpoint: None,
                udp_last_seen_unix_ms: None,
            },
        );

        let stream = guard
            .streams
            .entry(key.to_owned())
            .or_insert_with(StreamEntry::new);
        match role {
            ClientRole::Publisher => {
                stream.publisher_session = Some(session_id);
                stream.publisher_connections_total += 1;
            }
            ClientRole::Subscriber => {
                stream.subscribers.insert(session_id);
                stream.subscriber_connections_total += 1;
            }
        }

        stream.last_activity_unix_ms = now;

        Ok(SessionRegistration { session_id })
    }

    pub fn unregister_session(&self, session_id: SessionId, reason: &str) {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        let Some(session) = guard.sessions.remove(&session_id) else {
            return;
        };

        let mut remove_stream = false;
        if let Some(stream) = guard.streams.get_mut(&session.key) {
            match session.role {
                ClientRole::Publisher => {
                    if stream.publisher_session == Some(session_id) {
                        stream.publisher_session = None;
                    }
                    stream.last_publisher_disconnect_reason = Some(reason.to_owned());
                }
                ClientRole::Subscriber => {
                    stream.subscribers.remove(&session_id);
                    stream.last_subscriber_disconnect_reason = Some(reason.to_owned());
                }
            }

            stream.last_activity_unix_ms = unix_now_ms();
            remove_stream = stream.publisher_session.is_none() && stream.subscribers.is_empty();
        }

        if remove_stream {
            guard.streams.remove(&session.key);
        }
    }

    pub fn record_control_heartbeat(&self, session_id: SessionId) -> bool {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        let (role, key, now) = {
            let Some(session) = guard.sessions.get_mut(&session_id) else {
                return false;
            };

            let now = unix_now_ms();
            (session.role, session.key.clone(), now)
        };

        if let Some(stream) = guard.streams.get_mut(&key) {
            match role {
                ClientRole::Publisher => stream.publisher_tcp_heartbeats_total += 1,
                ClientRole::Subscriber => stream.subscriber_tcp_heartbeats_total += 1,
            }
            stream.last_activity_unix_ms = now;
        }

        true
    }

    pub fn record_udp_register(
        &self,
        session_id: SessionId,
        endpoint: SocketAddr,
    ) -> Result<(), UdpRegisterError> {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        let (role, key, now) = {
            let Some(session) = guard.sessions.get_mut(&session_id) else {
                return Err(UdpRegisterError::UnknownSession);
            };

            let now = unix_now_ms();
            session.udp_endpoint = Some(endpoint);
            session.udp_last_seen_unix_ms = Some(now);
            (session.role, session.key.clone(), now)
        };

        if let Some(stream) = guard.streams.get_mut(&key) {
            match role {
                ClientRole::Publisher => stream.publisher_udp_registers_total += 1,
                ClientRole::Subscriber => stream.subscriber_udp_registers_total += 1,
            }
            stream.last_activity_unix_ms = now;
        }

        Ok(())
    }

    pub fn record_udp_heartbeat(
        &self,
        session_id: SessionId,
        endpoint: SocketAddr,
    ) -> Result<(), UdpHeartbeatError> {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        let (role, key, now) = {
            let Some(session) = guard.sessions.get_mut(&session_id) else {
                return Err(UdpHeartbeatError::UnknownSession);
            };

            let now = unix_now_ms();
            ensure_udp_endpoint_matches(session, endpoint, now, self.udp_session_timeout_ms)
                .map_err(map_udp_validation_error_to_heartbeat_error)?;
            session.udp_last_seen_unix_ms = Some(now);
            (session.role, session.key.clone(), now)
        };

        if let Some(stream) = guard.streams.get_mut(&key) {
            match role {
                ClientRole::Publisher => stream.publisher_udp_heartbeats_total += 1,
                ClientRole::Subscriber => stream.subscriber_udp_heartbeats_total += 1,
            }
            stream.last_activity_unix_ms = now;
        }

        Ok(())
    }

    pub fn record_invalid_udp_packet(&self) {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        guard.invalid_udp_packets_total += 1;
    }

    pub fn record_unknown_udp_session(&self) {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        guard.unknown_udp_session_packets_total += 1;
    }

    pub fn prepare_audio_dispatch(
        &self,
        session_id: SessionId,
        source_endpoint: SocketAddr,
        payload_bytes: usize,
    ) -> Result<AudioDispatchPlan, AudioDispatchError> {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        let (key, now) = {
            let Some(session) = guard.sessions.get_mut(&session_id) else {
                return Err(AudioDispatchError::UnknownSession);
            };

            if session.role != ClientRole::Publisher {
                return Err(AudioDispatchError::WrongRole);
            }

            let now = unix_now_ms();
            ensure_udp_endpoint_matches(session, source_endpoint, now, self.udp_session_timeout_ms)
                .map_err(map_udp_validation_error_to_audio_dispatch_error)?;
            session.udp_last_seen_unix_ms = Some(now);
            (session.key.clone(), now)
        };

        let subscriber_ids = guard
            .streams
            .get(&key)
            .map(|stream| stream.subscribers.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();

        let mut targets = Vec::new();
        for subscriber_session_id in subscriber_ids {
            let Some(subscriber_session) = guard.sessions.get(&subscriber_session_id) else {
                continue;
            };

            if let Some(endpoint) = active_udp_endpoint(
                subscriber_session.udp_endpoint,
                subscriber_session.udp_last_seen_unix_ms,
                now,
                self.udp_session_timeout_ms,
            ) {
                targets.push(UdpDispatchTarget {
                    session_id: subscriber_session_id,
                    endpoint,
                });
            }
        }

        if let Some(stream) = guard.streams.get_mut(&key) {
            stream.udp_audio_packets_in_total += 1;
            stream.udp_audio_bytes_in_total += payload_bytes as u64;
            stream.last_activity_unix_ms = now;
        }

        Ok(AudioDispatchPlan {
            publisher_session_id: session_id,
            key,
            targets,
        })
    }

    pub fn record_udp_send_error(&self, session_id: SessionId) {
        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        let Some(key) = guard
            .sessions
            .get(&session_id)
            .map(|session| session.key.clone())
        else {
            return;
        };

        if let Some(stream) = guard.streams.get_mut(&key) {
            stream.udp_send_errors_total += 1;
            stream.last_activity_unix_ms = unix_now_ms();
        }
    }

    pub fn record_audio_dispatch_outcome(
        &self,
        key: &str,
        payload_bytes: usize,
        successful_targets: usize,
        send_errors: usize,
    ) {
        if successful_targets == 0 && send_errors == 0 {
            return;
        }

        let mut guard = self.inner.lock().expect("stream registry mutex poisoned");
        let Some(stream) = guard.streams.get_mut(key) else {
            return;
        };

        stream.udp_audio_packets_out_total += successful_targets as u64;
        stream.udp_audio_bytes_out_total += (payload_bytes as u64) * (successful_targets as u64);
        stream.udp_send_errors_total += send_errors as u64;
        stream.last_activity_unix_ms = unix_now_ms();
    }

    pub fn tcp_peer_addr(&self, session_id: SessionId) -> Option<String> {
        let guard = self.inner.lock().expect("stream registry mutex poisoned");
        guard
            .sessions
            .get(&session_id)
            .map(|session| session.tcp_peer_addr.clone())
    }

    pub fn session_matches(&self, session_id: SessionId, role: ClientRole, key: &str) -> bool {
        let guard = self.inner.lock().expect("stream registry mutex poisoned");
        matches!(
            guard.sessions.get(&session_id),
            Some(session) if session.role == role && session.key == key
        )
    }

    pub fn subscriber_target_is_active(&self, session_id: SessionId, endpoint: SocketAddr) -> bool {
        let guard = self.inner.lock().expect("stream registry mutex poisoned");
        let now = unix_now_ms();
        matches!(
            guard.sessions.get(&session_id),
            Some(session)
                if session.role == ClientRole::Subscriber
                    && active_udp_endpoint(
                        session.udp_endpoint,
                        session.udp_last_seen_unix_ms,
                        now,
                        self.udp_session_timeout_ms,
                    ) == Some(endpoint)
        )
    }

    pub fn session_role_and_key(&self, session_id: SessionId) -> Option<(ClientRole, String)> {
        let guard = self.inner.lock().expect("stream registry mutex poisoned");
        guard
            .sessions
            .get(&session_id)
            .map(|session| (session.role, session.key.clone()))
    }

    pub fn snapshot(&self) -> RegistrySnapshot {
        let guard = self.inner.lock().expect("stream registry mutex poisoned");
        let now = unix_now_ms();
        let mut streams = Vec::with_capacity(guard.streams.len());
        let mut active_publisher_count = 0;
        let mut active_subscriber_count = 0;
        let mut active_udp_publisher_count = 0;
        let mut active_udp_subscriber_count = 0;

        for (key, stream) in &guard.streams {
            let publisher_control_connected = stream.publisher_session.is_some();
            let publisher_udp_registered = stream
                .publisher_session
                .and_then(|session_id| guard.sessions.get(&session_id))
                .and_then(|session| {
                    active_udp_endpoint(
                        session.udp_endpoint,
                        session.udp_last_seen_unix_ms,
                        now,
                        self.udp_session_timeout_ms,
                    )
                })
                .is_some();

            let mut subscriber_udp_registered_count = 0;
            for session_id in &stream.subscribers {
                if let Some(session) = guard.sessions.get(session_id)
                    && active_udp_endpoint(
                        session.udp_endpoint,
                        session.udp_last_seen_unix_ms,
                        now,
                        self.udp_session_timeout_ms,
                    )
                    .is_some()
                {
                    subscriber_udp_registered_count += 1;
                }
            }

            if publisher_control_connected {
                active_publisher_count += 1;
            }
            active_subscriber_count += stream.subscribers.len();
            if publisher_udp_registered {
                active_udp_publisher_count += 1;
            }
            active_udp_subscriber_count += subscriber_udp_registered_count;

            streams.push(StreamSnapshot {
                key: key.clone(),
                publisher_control_connected,
                publisher_udp_registered,
                subscriber_count: stream.subscribers.len(),
                subscriber_udp_registered_count,
                publisher_connections_total: stream.publisher_connections_total,
                subscriber_connections_total: stream.subscriber_connections_total,
                publisher_tcp_heartbeats_total: stream.publisher_tcp_heartbeats_total,
                subscriber_tcp_heartbeats_total: stream.subscriber_tcp_heartbeats_total,
                publisher_udp_registers_total: stream.publisher_udp_registers_total,
                subscriber_udp_registers_total: stream.subscriber_udp_registers_total,
                publisher_udp_heartbeats_total: stream.publisher_udp_heartbeats_total,
                subscriber_udp_heartbeats_total: stream.subscriber_udp_heartbeats_total,
                udp_audio_packets_in_total: stream.udp_audio_packets_in_total,
                udp_audio_bytes_in_total: stream.udp_audio_bytes_in_total,
                udp_audio_packets_out_total: stream.udp_audio_packets_out_total,
                udp_audio_bytes_out_total: stream.udp_audio_bytes_out_total,
                udp_send_errors_total: stream.udp_send_errors_total,
                last_activity_unix_ms: stream.last_activity_unix_ms,
                last_publisher_disconnect_reason: stream.last_publisher_disconnect_reason.clone(),
                last_subscriber_disconnect_reason: stream.last_subscriber_disconnect_reason.clone(),
            });
        }

        streams.sort_by(|left, right| left.key.cmp(&right.key));

        RegistrySnapshot {
            generated_at_unix_ms: now,
            stream_count: streams.len(),
            active_publisher_count,
            active_subscriber_count,
            active_udp_publisher_count,
            active_udp_subscriber_count,
            invalid_udp_packets_total: guard.invalid_udp_packets_total,
            unknown_udp_session_packets_total: guard.unknown_udp_session_packets_total,
            streams,
        }
    }
}

impl StreamEntry {
    fn new() -> Self {
        Self {
            publisher_session: None,
            subscribers: HashSet::new(),
            publisher_connections_total: 0,
            subscriber_connections_total: 0,
            publisher_tcp_heartbeats_total: 0,
            subscriber_tcp_heartbeats_total: 0,
            publisher_udp_registers_total: 0,
            subscriber_udp_registers_total: 0,
            publisher_udp_heartbeats_total: 0,
            subscriber_udp_heartbeats_total: 0,
            udp_audio_packets_in_total: 0,
            udp_audio_bytes_in_total: 0,
            udp_audio_packets_out_total: 0,
            udp_audio_bytes_out_total: 0,
            udp_send_errors_total: 0,
            last_activity_unix_ms: unix_now_ms(),
            last_publisher_disconnect_reason: None,
            last_subscriber_disconnect_reason: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UdpEndpointValidationError {
    NotRegistered,
    EndpointMismatch,
    EndpointExpired,
}

fn active_udp_endpoint(
    endpoint: Option<SocketAddr>,
    last_seen_unix_ms: Option<u64>,
    now: u64,
    udp_session_timeout_ms: u64,
) -> Option<SocketAddr> {
    match (endpoint, last_seen_unix_ms) {
        (Some(endpoint), Some(last_seen))
            if now.saturating_sub(last_seen) <= udp_session_timeout_ms =>
        {
            Some(endpoint)
        }
        _ => None,
    }
}

fn ensure_udp_endpoint_matches(
    session: &SessionEntry,
    source_endpoint: SocketAddr,
    now: u64,
    udp_session_timeout_ms: u64,
) -> Result<(), UdpEndpointValidationError> {
    let Some(registered_endpoint) = session.udp_endpoint else {
        return Err(UdpEndpointValidationError::NotRegistered);
    };
    if registered_endpoint != source_endpoint {
        return Err(UdpEndpointValidationError::EndpointMismatch);
    }

    let Some(last_seen) = session.udp_last_seen_unix_ms else {
        return Err(UdpEndpointValidationError::NotRegistered);
    };
    if now.saturating_sub(last_seen) > udp_session_timeout_ms {
        return Err(UdpEndpointValidationError::EndpointExpired);
    }

    Ok(())
}

fn map_udp_validation_error_to_audio_dispatch_error(
    err: UdpEndpointValidationError,
) -> AudioDispatchError {
    match err {
        UdpEndpointValidationError::NotRegistered => AudioDispatchError::UdpNotRegistered,
        UdpEndpointValidationError::EndpointMismatch => AudioDispatchError::UdpEndpointMismatch,
        UdpEndpointValidationError::EndpointExpired => AudioDispatchError::UdpEndpointExpired,
    }
}

fn map_udp_validation_error_to_heartbeat_error(
    err: UdpEndpointValidationError,
) -> UdpHeartbeatError {
    match err {
        UdpEndpointValidationError::NotRegistered => UdpHeartbeatError::UdpNotRegistered,
        UdpEndpointValidationError::EndpointMismatch => UdpHeartbeatError::UdpEndpointMismatch,
        UdpEndpointValidationError::EndpointExpired => UdpHeartbeatError::UdpEndpointExpired,
    }
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::thread::sleep;
    use std::time::Duration;

    use crate::protocol::{ClientRole, SessionId};

    use super::{
        AudioDispatchError, RegisterSessionError, StreamRegistry, UdpHeartbeatError,
        UdpRegisterError,
    };

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
    }

    #[test]
    fn rejects_second_publisher_on_same_key() {
        let registry = StreamRegistry::new(15_000);
        assert!(
            registry
                .register_control_session(ClientRole::Publisher, "abc", "peer-a".to_owned())
                .is_ok()
        );
        assert!(matches!(
            registry.register_control_session(ClientRole::Publisher, "abc", "peer-b".to_owned()),
            Err(RegisterSessionError::PublisherAlreadyConnected)
        ));
    }

    #[test]
    fn allows_new_publisher_after_disconnect() {
        let registry = StreamRegistry::new(15_000);
        let session = registry
            .register_control_session(ClientRole::Publisher, "abc", "peer-a".to_owned())
            .unwrap();
        registry.unregister_session(session.session_id, "peer_disconnected");
        assert!(
            registry
                .register_control_session(ClientRole::Publisher, "abc", "peer-b".to_owned())
                .is_ok()
        );
    }

    #[test]
    fn dispatches_audio_only_to_udp_registered_subscribers() {
        let registry = StreamRegistry::new(15_000);
        let publisher = registry
            .register_control_session(ClientRole::Publisher, "room", "publisher".to_owned())
            .unwrap();
        let subscriber_a = registry
            .register_control_session(ClientRole::Subscriber, "room", "subscriber-a".to_owned())
            .unwrap();
        let _subscriber_b = registry
            .register_control_session(ClientRole::Subscriber, "room", "subscriber-b".to_owned())
            .unwrap();

        assert_eq!(
            registry.record_udp_register(publisher.session_id, addr(4000)),
            Ok(())
        );
        assert_eq!(
            registry.record_udp_register(subscriber_a.session_id, addr(5000)),
            Ok(())
        );

        let targets = registry
            .prepare_audio_dispatch(publisher.session_id, addr(4000), 64)
            .unwrap()
            .targets;

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].session_id, subscriber_a.session_id);
        assert_eq!(targets[0].endpoint, addr(5000));
    }

    #[test]
    fn rejects_audio_from_unregistered_publisher_endpoint() {
        let registry = StreamRegistry::new(15_000);
        let publisher = registry
            .register_control_session(ClientRole::Publisher, "room", "publisher".to_owned())
            .unwrap();

        assert_eq!(
            registry.prepare_audio_dispatch(publisher.session_id, addr(4000), 64),
            Err(AudioDispatchError::UdpNotRegistered)
        );
    }

    #[test]
    fn rejects_audio_from_subscriber_session() {
        let registry = StreamRegistry::new(15_000);
        let subscriber = registry
            .register_control_session(ClientRole::Subscriber, "room", "subscriber".to_owned())
            .unwrap();

        assert_eq!(
            registry.prepare_audio_dispatch(subscriber.session_id, addr(5000), 64),
            Err(AudioDispatchError::WrongRole)
        );
    }

    #[test]
    fn udp_heartbeat_does_not_rebind_endpoint() {
        let registry = StreamRegistry::new(15_000);
        let subscriber = registry
            .register_control_session(ClientRole::Subscriber, "room", "subscriber".to_owned())
            .unwrap();

        assert_eq!(
            registry.record_udp_register(subscriber.session_id, addr(5000)),
            Ok(())
        );
        assert_eq!(
            registry.record_udp_heartbeat(subscriber.session_id, addr(5001)),
            Err(UdpHeartbeatError::UdpEndpointMismatch)
        );
        assert!(registry.subscriber_target_is_active(subscriber.session_id, addr(5000)));
        assert!(!registry.subscriber_target_is_active(subscriber.session_id, addr(5001)));
    }

    #[test]
    fn udp_heartbeat_requires_register_after_timeout() {
        let registry = StreamRegistry::new(1);
        let publisher = registry
            .register_control_session(ClientRole::Publisher, "room", "publisher".to_owned())
            .unwrap();

        assert_eq!(
            registry.record_udp_register(publisher.session_id, addr(4000)),
            Ok(())
        );
        sleep(Duration::from_millis(5));
        assert_eq!(
            registry.record_udp_heartbeat(publisher.session_id, addr(4000)),
            Err(UdpHeartbeatError::UdpEndpointExpired)
        );
        assert_eq!(
            registry.prepare_audio_dispatch(publisher.session_id, addr(4000), 64),
            Err(AudioDispatchError::UdpEndpointExpired)
        );
        assert_eq!(
            registry.record_udp_register(publisher.session_id, addr(4000)),
            Ok(())
        );
    }

    #[test]
    fn rejects_unknown_udp_register() {
        let registry = StreamRegistry::new(15_000);
        assert_eq!(
            registry.record_udp_register(SessionId::from_bytes([0x77; 16]), addr(4000)),
            Err(UdpRegisterError::UnknownSession)
        );
    }

    #[test]
    fn snapshot_contains_udp_counters() {
        let registry = StreamRegistry::new(15_000);
        let publisher = registry
            .register_control_session(ClientRole::Publisher, "room", "publisher".to_owned())
            .unwrap();
        let subscriber = registry
            .register_control_session(ClientRole::Subscriber, "room", "subscriber".to_owned())
            .unwrap();

        assert!(registry.record_control_heartbeat(publisher.session_id));
        assert!(registry.record_control_heartbeat(subscriber.session_id));
        assert_eq!(
            registry.record_udp_register(publisher.session_id, addr(4100)),
            Ok(())
        );
        assert_eq!(
            registry.record_udp_register(subscriber.session_id, addr(5100)),
            Ok(())
        );
        assert_eq!(
            registry.record_udp_heartbeat(publisher.session_id, addr(4100)),
            Ok(())
        );
        assert_eq!(
            registry.record_udp_heartbeat(subscriber.session_id, addr(5100)),
            Ok(())
        );
        let plan = registry
            .prepare_audio_dispatch(publisher.session_id, addr(4100), 12)
            .unwrap();
        registry.record_audio_dispatch_outcome(&plan.key, 12, plan.targets.len(), 0);

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.stream_count, 1);
        assert_eq!(snapshot.active_publisher_count, 1);
        assert_eq!(snapshot.active_subscriber_count, 1);
        assert_eq!(snapshot.active_udp_publisher_count, 1);
        assert_eq!(snapshot.active_udp_subscriber_count, 1);

        let stream = &snapshot.streams[0];
        assert_eq!(stream.key, "room");
        assert_eq!(stream.publisher_connections_total, 1);
        assert_eq!(stream.subscriber_connections_total, 1);
        assert_eq!(stream.publisher_udp_registers_total, 1);
        assert_eq!(stream.subscriber_udp_registers_total, 1);
        assert_eq!(stream.udp_audio_packets_in_total, 1);
        assert_eq!(stream.udp_audio_bytes_in_total, 12);
        assert_eq!(stream.udp_audio_packets_out_total, 1);
        assert_eq!(stream.udp_audio_bytes_out_total, 12);
    }

    #[test]
    fn preserves_session_peer_lookup() {
        let registry = StreamRegistry::new(15_000);
        let session = registry
            .register_control_session(ClientRole::Subscriber, "room", "peer-1".to_owned())
            .unwrap();

        assert_eq!(
            registry.tcp_peer_addr(session.session_id),
            Some("peer-1".to_owned())
        );
        assert_eq!(
            registry.session_role_and_key(session.session_id),
            Some((ClientRole::Subscriber, "room".to_owned()))
        );
        assert_eq!(
            SessionId::from_hex(&session.session_id.to_hex()).unwrap(),
            session.session_id
        );
    }
}
