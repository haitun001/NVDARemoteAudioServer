# NVDARemoteAudioServer API

This document describes the wire protocol used by `NVDARemoteAudioServer`.
It is intended for client developers who need to integrate with the server.

The server is not an HTTP service. It exposes:

- A TCP control API for authentication and heartbeats.
- A UDP binary API for endpoint registration, UDP heartbeats, and audio packet forwarding.
- A TCP status API for operational snapshots.

The server does not capture, encode, decode, play, mix, resample, retransmit, reorder, or repair audio. Clients own all audio work.

## Default Endpoints

| Purpose | Transport | Default address |
| --- | --- | --- |
| Control handshake and control heartbeat | TCP | `0.0.0.0:6838` |
| UDP registration, UDP heartbeat, audio data | UDP | `0.0.0.0:6838` |
| Status snapshot | TCP | `0.0.0.0:6839` |

Startup arguments:

```bash
NVDARemoteAudioServer --port=6838 --sport=6839 --log=/home/app/NVDARemoteAudioServer/logs/NVDARemoteAudioServer.log
```

| Argument | Meaning |
| --- | --- |
| `--port=6838` | Sets both the TCP control port and UDP data port. |
| `--sport=6839` | Sets the TCP status port. |
| `--log=/path/to/file.log` | Writes logs to a file. Without it, logs go to stdout. |

## Protocol Constants

| Name | Value |
| --- | --- |
| TCP handshake max request size | `4096` bytes |
| TCP control message max request size | `1024` bytes |
| TCP status request max size | `1024` bytes |
| TCP status response max size used by bundled tooling | `16777216` bytes |
| TCP handshake timeout | `5000ms` |
| TCP control idle timeout | `15000ms` |
| TCP heartbeat interval returned to clients | `5000ms` |
| UDP session timeout | `15000ms` |
| UDP max packet size | `1400` bytes |
| UDP max audio payload size | `1200` bytes |
| UDP magic | ASCII `RAS1` |
| UDP version | `1` |
| Status access key | `audiostatus` |

## Key Rules

The business `key` is the password/channel string used to bind one publisher and many subscribers together.

Rules:

- Must be non-empty.
- Must be at most `128` UTF-8 bytes.
- Is matched exactly.
- Is not trimmed, lowercased, normalized, or restricted to ASCII.
- May contain printable spaces, symbols, and Unicode characters.
- Must not contain control characters such as newline, tab, escape, or other Unicode control code points.

Client integration note: encode the key as a normal JSON string. Let a JSON encoder escape quotes, backslashes, Unicode, and other required characters.

## TCP Control API

Every publisher and every subscriber must keep its own TCP control connection open for the full session lifetime.

If the TCP control connection closes, the server immediately invalidates the session and its UDP endpoint.

### Handshake Request

After connecting to the control port, send exactly one UTF-8 JSON object followed by `\n`.

Publisher:

```json
{"role":"publisher","key":"room-123"}
```

Subscriber:

```json
{"role":"subscriber","key":"room-123"}
```

Fields:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `role` | string | yes | `publisher` or `subscriber`. |
| `key` | string | yes | Business key/password/channel string. |

Request framing:

- The JSON request is line-oriented and must end with `\n`.
- `\r\n` is accepted because `\r` is ignored while reading a line.
- Empty lines are rejected.
- Requests larger than `4096` bytes are rejected.
- If a client connects and does not finish the handshake within `5000ms`, the handshake fails.

### Successful Handshake Response

The server replies with one JSON line:

```json
{"status":"ok","message":"control session established","role":"publisher","key":"room-123","session_id":"00112233445566778899aabbccddeeff","udp_port":6838,"tcp_heartbeat_interval_ms":5000,"udp_session_timeout_ms":15000,"udp_audio_payload_max_bytes":1200}
```

Fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `status` | string | `ok` on success. |
| `message` | string | Human-readable status message. |
| `role` | string | The accepted role. |
| `key` | string | The accepted key. |
| `session_id` | string | 32 lowercase hexadecimal characters representing 16 raw bytes. |
| `udp_port` | number | UDP port to use for register, UDP heartbeat, and audio. |
| `tcp_heartbeat_interval_ms` | number | Recommended TCP heartbeat interval. |
| `udp_session_timeout_ms` | number | UDP endpoint inactivity timeout. |
| `udp_audio_payload_max_bytes` | number | Max audio payload bytes in one UDP audio packet. |

Client integration note: convert the 32-character `session_id` hex string into 16 raw bytes before using it in UDP packets.

### Handshake Failure Behavior

If the same `key` already has an active publisher, a second publisher is rejected with one JSON line:

```json
{"status":"error","message":"publisher already connected for this key","key":"room-123"}
```

Other malformed handshakes, invalid keys, invalid JSON, oversized requests, or timeout cases are not a stable JSON error API. Treat them as connection failure and reconnect only after fixing the request.

### TCP Control Heartbeat

After a successful handshake, send this JSON line periodically on the same TCP connection:

```json
{"type":"heartbeat"}
```

Behavior:

- The server does not send a heartbeat response.
- Use `tcp_heartbeat_interval_ms` from the handshake response as the normal interval.
- If no valid control message is received within `15000ms`, the server closes the session.
- A TCP connection is bound to exactly one role and one key.

## UDP Binary API

All UDP packets start with the same base header.

### Base Header

| Offset | Size | Field | Value |
| --- | ---: | --- | --- |
| `0` | `4` | magic | ASCII `RAS1` |
| `4` | `1` | version | `0x01` |
| `5` | `1` | packet_type | See packet type table. |
| `6` | `16` | session_id | 16 raw bytes from the TCP handshake `session_id`. |

Base header length: `22` bytes.

Packet types:

| Type | Name | Direction |
| --- | --- | --- |
| `0x01` | `register` | Client to server |
| `0x02` | `register_ack` | Server to client |
| `0x03` | `heartbeat` | Client to server |
| `0x04` | `audio_data` | Publisher to server, server to subscribers |

Packets with invalid magic, unsupported version, unknown type, invalid length, or payload above the configured limit are rejected and counted as invalid UDP packets.

### UDP Register

Register packet layout:

| Offset | Size | Field |
| --- | ---: | --- |
| `0` | `4` | magic `RAS1` |
| `4` | `1` | version `0x01` |
| `5` | `1` | packet_type `0x01` |
| `6` | `16` | session_id |

Total size: `22` bytes.

After a successful TCP handshake, every publisher and subscriber must send `register` from its UDP socket to the server UDP port.

Server behavior:

- If `session_id` is known and its TCP control session is still alive, the server binds the session to the UDP source IP and source port.
- The server replies to the same UDP endpoint with `register_ack`.
- If `session_id` is unknown or the TCP session has already ended, registration is rejected and no `register_ack` is sent.
- A later valid `register` from the same live session updates the UDP endpoint. This is how clients recover after their UDP source port changes.

### UDP Register Ack

Register ack packet layout:

| Offset | Size | Field |
| --- | ---: | --- |
| `0` | `4` | magic `RAS1` |
| `4` | `1` | version `0x01` |
| `5` | `1` | packet_type `0x02` |
| `6` | `16` | session_id |

Total size: `22` bytes.

Client behavior:

- Do not send audio as a publisher until `register_ack` has been received.
- Do not treat a subscriber as ready to receive until `register_ack` has been received.
- If the UDP source port changes, send `register` again and wait for a new `register_ack`.

### UDP Heartbeat

Heartbeat packet layout:

| Offset | Size | Field |
| --- | ---: | --- |
| `0` | `4` | magic `RAS1` |
| `4` | `1` | version `0x01` |
| `5` | `1` | packet_type `0x03` |
| `6` | `16` | session_id |

Total size: `22` bytes.

Behavior:

- Publishers and subscribers should send UDP heartbeat packets periodically.
- The server does not send a UDP heartbeat response.
- The heartbeat must come from the currently registered UDP source endpoint.
- If the registered UDP endpoint is inactive for `15000ms`, it is treated as expired.
- After expiry or source port change, send `register` again and wait for `register_ack`.

### UDP Audio Data

Audio packet layout:

| Offset | Size | Field | Encoding |
| --- | ---: | --- | --- |
| `0` | `4` | magic | ASCII `RAS1` |
| `4` | `1` | version | `0x01` |
| `5` | `1` | packet_type | `0x04` |
| `6` | `16` | session_id | 16 raw bytes |
| `22` | `8` | sequence | big-endian `u64` |
| `30` | `8` | timestamp_ms | big-endian `u64` |
| `38` | `N` | payload | opaque client-defined audio bytes |

Minimum size: `38` bytes.

Maximum payload size: `1200` bytes.

Maximum UDP packet size used by the server receive buffer: `1400` bytes.

Publisher-to-server rules:

- Only a `publisher` session may send `audio_data`.
- The TCP control session must still be alive.
- The publisher UDP endpoint must have successfully registered.
- The packet must come from the same UDP source IP and port that registered the session.
- `session_id` must be the publisher session id.
- `sequence`, `timestamp_ms`, and `payload` are opaque to the server except for size and layout checks.

Server-to-subscriber forwarding rules:

- The server forwards only to active subscriber UDP endpoints registered for the same `key`.
- The forwarded packet keeps `sequence` unchanged.
- The forwarded packet keeps `timestamp_ms` unchanged.
- The forwarded packet keeps `payload` unchanged.
- The forwarded packet replaces `session_id` with the target subscriber session id.

The server does not acknowledge audio packets and does not retransmit dropped packets.

## Required Client Flows

### Publisher Flow

1. Open TCP connection to the control port.
2. Send handshake JSON line: `{"role":"publisher","key":"..."}`.
3. Read one JSON line response.
4. Verify `status == "ok"`.
5. Save `session_id`, `udp_port`, `tcp_heartbeat_interval_ms`, `udp_session_timeout_ms`, and `udp_audio_payload_max_bytes`.
6. Open a UDP socket.
7. Send UDP `register` using the 16-byte session id.
8. Wait for `register_ack`.
9. Keep sending TCP heartbeat on the TCP control connection.
10. Keep sending UDP heartbeat from the registered UDP socket.
11. Send UDP `audio_data` from the same UDP socket.

### Subscriber Flow

1. Open TCP connection to the control port.
2. Send handshake JSON line: `{"role":"subscriber","key":"..."}`.
3. Read one JSON line response.
4. Verify `status == "ok"`.
5. Save `session_id`, `udp_port`, `tcp_heartbeat_interval_ms`, and `udp_session_timeout_ms`.
6. Open a UDP socket.
7. Send UDP `register` using the 16-byte session id.
8. Wait for `register_ack`.
9. Keep sending TCP heartbeat on the TCP control connection.
10. Keep sending UDP heartbeat from the registered UDP socket.
11. Read UDP `audio_data`.
12. Verify incoming forwarded `audio_data` uses the subscriber's own session id.

## Status API

The status API uses a separate TCP port.

Send one JSON line:

```json
{"key":"audiostatus"}
```

Successful response is one JSON line containing a `RegistrySnapshot` object:

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

Invalid status key response:

```json
{"status":"error","message":"invalid status key"}
```

Invalid status request response:

```json
{"status":"error","message":"invalid status request"}
```

Status client requirements:

- Send exactly one JSON line ending in `\n`.
- Request size must not exceed `1024` bytes.
- Read one full JSON line until newline.
- Do not assume the response fits in 4 KB. Large deployments can produce much larger snapshots.

## Error And Recovery Guidance

Recommended client behavior:

- If TCP control disconnects, treat the whole session as invalid and start from TCP handshake again.
- If UDP `register_ack` is not received, retry `register` with backoff while keeping TCP alive.
- If the UDP source port changes, send `register` again and wait for `register_ack`.
- If subscriber audio stops but TCP is still alive, continue TCP and UDP heartbeat, and re-register UDP if the client suspects NAT or port changes.
- If the server rejects or ignores UDP audio, verify role, session id, and UDP source endpoint binding.

## Minimal UDP Encoding Reference

Pseudo-code for a UDP audio packet:

```text
packet = bytes()
packet += ASCII("RAS1")
packet += u8(1)
packet += u8(0x04)
packet += session_id_16_bytes
packet += u64_be(sequence)
packet += u64_be(timestamp_ms)
packet += payload
```

Pseudo-code for a UDP register packet:

```text
packet = bytes()
packet += ASCII("RAS1")
packet += u8(1)
packet += u8(0x01)
packet += session_id_16_bytes
```
