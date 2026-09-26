# NVDARemoteAudioServer API

[简体中文](API-ZHCN.md) · [Deployment and usage](README.md)

These interfaces use TCP/UDP directly, not HTTP.

## Endpoints and limits

| Interface | Default listener | Purpose |
| --- | --- | --- |
| TCP control | `0.0.0.0:6838` | Handshake and session heartbeats |
| UDP data | `0.0.0.0:6838` | Endpoint registration, heartbeats, and audio |
| TCP status | `0.0.0.0:6839` | Status queries |

| Limit | Value |
| --- | --- |
| TCP handshake request | 4096 bytes |
| TCP control message | 1024 bytes |
| TCP status request | 1024 bytes |
| Handshake and status request timeout | 5000 ms |
| TCP control idle timeout | 15000 ms |
| Recommended TCP heartbeat interval | 5000 ms |
| UDP endpoint inactivity timeout | 15000 ms |
| UDP receive buffer per packet | 1400 bytes |
| Audio payload limit | 1200 bytes |

TCP requests and responses are UTF-8 JSON objects, one per line, terminated by `\n`. Requests also accept `\r\n`. Size limits exclude the terminating newline and ignored carriage returns. Empty lines are invalid.

## Keys and streams

`key` is the channel's shared password, matched exactly as in NVDA Remote:

- Non-empty, at most 128 UTF-8 bytes.
- Spaces, symbols, and Unicode are allowed. No trimming, case conversion, or Unicode normalization is applied.
- Control characters, including newline, tab, and escape, are rejected.

The required `stream` field accepts these values:

| Value | Direction |
| --- | --- |
| `system_audio` | Controlled device's system audio → controller |
| `voice_controlled_to_controller` | Controlled device's microphone → controller |
| `voice_controller_to_controlled` | Controller's microphone → controlled device |

Each `(key, stream)` allows at most one publisher and multiple subscribers. Subscribers may connect first. A publisher occupies its slot until its TCP session ends, regardless of UDP registration or expiry.

## TCP control API

Each publisher and subscriber keeps a TCP connection bound to one role, key, and stream. A detected disconnect or control timeout immediately invalidates the session and its UDP endpoint. Reconnecting requires a new handshake.

### Handshake

Send the handshake immediately after connecting and complete it within 5000 ms. `role`, `key`, and `stream` are required strings; `role` is `publisher` or `subscriber`.

```json
{"role":"publisher","key":"room-123","stream":"system_audio"}
```

Successful response:

```json
{"status":"ok","message":"control session established","role":"publisher","key":"room-123","stream":"system_audio","session_id":"00112233445566778899aabbccddeeff","udp_port":6838,"tcp_heartbeat_interval_ms":5000,"udp_session_timeout_ms":15000,"udp_audio_payload_max_bytes":1200}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `status` | string | `ok` on success |
| `message` | string | Status description |
| `role`, `key`, `stream` | string | Accepted handshake values |
| `session_id` | string | 16 bytes encoded as 32 lowercase hexadecimal characters |
| `udp_port` | integer | Server port for all UDP packets |
| `tcp_heartbeat_interval_ms` | integer | Recommended TCP heartbeat interval |
| `udp_session_timeout_ms` | integer | UDP endpoint inactivity timeout |
| `udp_audio_payload_max_bytes` | integer | Maximum audio payload per packet |

If a publisher already exists for the same `(key, stream)`, the server returns this error and closes the connection:

```json
{"status":"error","message":"publisher already connected for this key and stream","key":"room-123","stream":"system_audio"}
```

For other invalid handshakes or timeouts, the server closes the connection without a JSON error response.

### Heartbeats

Send this JSON line on the same connection at the interval returned in the handshake:

```json
{"type":"heartbeat"}
```

There is no response. The server closes the session if it does not receive a complete valid heartbeat within 15000 ms. Invalid or oversized control messages also close the session. UDP traffic does not keep the TCP session alive.

## UDP binary API

### Common header

All offsets and sizes below are in bytes. Every packet begins with this 22-byte header:

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| 0 | 4 | `magic` | ASCII `RAS1` |
| 4 | 1 | `version` | `0x01` |
| 5 | 1 | `packet_type` | See below |
| 6 | 16 | `session_id` | Raw bytes from the TCP handshake |

| Type | Name | Direction | Total size |
| --- | --- | --- | --- |
| `0x01` | `register` | Client → server | 22 bytes |
| `0x02` | `register_ack` | Server → client | 22 bytes |
| `0x03` | `heartbeat` | Client → server | 22 bytes |
| `0x04` | `audio_data` | Publisher → server → subscribers | 38–1238 bytes |

The first three packet types contain only the common header. Invalid packets are dropped; error counters are described in the status API.

### Endpoint registration

After the TCP handshake, send `register` to `udp_port` from the socket that will carry the session's UDP traffic. The server binds the source IP address and port and replies with `register_ack` containing the same session ID. Unknown or ended sessions receive no acknowledgment.

Wait for a matching `register_ack` before using the endpoint. If none arrives, retry with backoff while keeping TCP alive. A new registration replaces the endpoint; register again after expiry or a source address or port change. Heartbeats and audio cannot change the binding.

### UDP heartbeats and expiry

Both roles should send UDP `heartbeat` packets periodically from their registered socket, for example every 5000 ms. There is no response.

A valid registration, UDP heartbeat, or publisher audio packet refreshes UDP activity. Forwarded audio and TCP heartbeats do not; subscribers must send their own UDP heartbeats.

An endpoint expires after more than 15000 ms without valid UDP activity. Until it registers again, the server rejects its heartbeats and audio and stops forwarding audio to it. UDP expiry does not end the TCP session.

### Audio packets

`audio_data` adds the following fields to the common header:

| Offset | Size | Field | Encoding |
| --- | ---: | --- | --- |
| 22 | 8 | `sequence` | Big-endian unsigned 64-bit integer |
| 30 | 8 | `timestamp_ms` | Big-endian unsigned 64-bit integer, milliseconds |
| 38 | 0–1200 | `payload` | Client-defined audio bytes |

Only a live `publisher` session may send audio, from its registered, unexpired source IP address and port.

The server forwards audio only to registered, unexpired subscribers on the same `(key, stream)`. It preserves `sequence`, `timestamp_ms`, and `payload`, and replaces `session_id` with the recipient's ID, which subscribers should verify. Audio format and sequence/timestamp progression are not checked.

Audio packets receive no acknowledgment and are not retransmitted. Packets can be dropped when the dispatch queue is full; delivery and ordering are not guaranteed.

## Status API

Send this request to the status port. The access key is fixed:

```json
{"key":"audiostatus"}
```

The server returns one JSON line and closes the connection. Responses contain channel keys; restrict access to trusted hosts.

Successful response (expanded for readability):

```json
{
  "generated_at_unix_ms": 1770000000000,
  "stream_count": 1,
  "active_publisher_count": 1,
  "active_subscriber_count": 2,
  "active_udp_publisher_count": 1,
  "active_udp_subscriber_count": 2,
  "invalid_udp_packets_total": 0,
  "unknown_udp_session_packets_total": 0,
  "streams": [
    {
      "key": "room-123",
      "stream": "system_audio",
      "publisher_control_connected": true,
      "publisher_udp_registered": true,
      "subscriber_count": 2,
      "subscriber_udp_registered_count": 2,
      "publisher_connections_total": 1,
      "subscriber_connections_total": 2,
      "publisher_tcp_heartbeats_total": 10,
      "subscriber_tcp_heartbeats_total": 20,
      "publisher_udp_registers_total": 1,
      "subscriber_udp_registers_total": 2,
      "publisher_udp_heartbeats_total": 10,
      "subscriber_udp_heartbeats_total": 20,
      "udp_audio_packets_in_total": 100,
      "udp_audio_bytes_in_total": 120000,
      "udp_audio_packets_out_total": 200,
      "udp_audio_bytes_out_total": 240000,
      "udp_send_errors_total": 0,
      "last_activity_unix_ms": 1770000000000
    }
  ]
}
```

### Snapshot fields

| Field | Meaning |
| --- | --- |
| `generated_at_unix_ms` | Snapshot time, Unix milliseconds |
| `stream_count`, `streams` | Current `(key, stream)` entries; one key can have up to three |
| `active_publisher_count`, `active_subscriber_count` | Sessions with a live TCP control connection |
| `active_udp_publisher_count`, `active_udp_subscriber_count` | Sessions with a registered, unexpired UDP endpoint |
| `invalid_udp_packets_total` | Malformed UDP packets and unexpected client-sent `register_ack` packets |
| `unknown_udp_session_packets_total` | Register, heartbeat, or audio packets with an unknown session ID |

The two global UDP error counters accumulate for the server process lifetime. Wrong-role, unregistered, mismatched, or expired endpoint rejections are logged but do not increment those counters.

### Per-stream fields

| Field | Meaning |
| --- | --- |
| `key`, `stream` | Routing pair |
| `publisher_control_connected`, `subscriber_count` | Current publisher presence and subscriber count over TCP |
| `publisher_udp_registered`, `subscriber_udp_registered_count` | Current unexpired UDP registrations |
| `publisher_connections_total`, `subscriber_connections_total` | Control session registrations |
| `publisher_tcp_heartbeats_total`, `subscriber_tcp_heartbeats_total` | Accepted TCP heartbeats |
| `publisher_udp_registers_total`, `subscriber_udp_registers_total` | Accepted UDP registrations, including re-registration |
| `publisher_udp_heartbeats_total`, `subscriber_udp_heartbeats_total` | Accepted UDP heartbeats |
| `udp_audio_packets_in_total`, `udp_audio_bytes_in_total` | Accepted publisher audio packets and payload bytes |
| `udp_audio_packets_out_total`, `udp_audio_bytes_out_total` | Successfully sent audio packets and payload bytes, counted per recipient |
| `udp_send_errors_total` | Registration acknowledgment send failures and audio forwarding errors; queue overflow counts each skipped recipient |
| `last_activity_unix_ms` | Last recorded stream activity, Unix milliseconds |
| `last_publisher_disconnect_reason`, `last_subscriber_disconnect_reason` | Latest disconnect reason for each role, omitted if none; common values are `peer_disconnected`, `timeout`, and `protocol_error` |

Byte counters exclude protocol headers. Successful UDP sends do not confirm receipt. A stream and its counters are removed when its last TCP session disconnects.

### Errors and response size

Wrong access key:

```json
{"status":"error","message":"invalid status key"}
```

Malformed, oversized, or timed-out request:

```json
{"status":"error","message":"invalid status request"}
```

Read through the newline. The server has no fixed response size limit; the load test reader allows 16 MiB (16777216 bytes).
