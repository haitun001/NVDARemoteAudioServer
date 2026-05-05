# Agent Maintenance Guide

This file defines the maintenance contract for agents working on `NVDARemoteAudioServer`. Follow it before changing code, docs, CI, or deployment files.

## Project Purpose

`NVDARemoteAudioServer` is a Rust relay server for low-latency remote audio transport.

The server is intentionally narrow:

- Authenticate clients through TCP control sessions.
- Keep TCP control heartbeats.
- Route streams by business `key`.
- Enforce one publisher per `key`.
- Allow many subscribers per `key`.
- Register UDP endpoints by `session_id`.
- Forward UDP audio packets from one publisher to active subscribers.
- Expose status statistics on a separate TCP status port.
- Write operational logs.

The server must not perform audio capture, playback, encoding, decoding, resampling, mixing, retransmission, RTP/RTCP, packet ordering repair, or codec-specific work. Those jobs belong to clients.

## Stable Protocol Contract

Do not change this contract unless the README, tests, load test, and downstream clients are updated together.

- Default TCP control port: `6838`.
- Default UDP data port: `6838`.
- Default TCP status port: `6839`.
- TCP handshake max request size: `4096` bytes.
- TCP control message max request size: `1024` bytes.
- TCP status request max size: `1024` bytes.
- Handshake timeout: `5000ms`.
- TCP control idle timeout: `15000ms`.
- UDP session timeout: `15000ms`.
- UDP max packet size: `1400` bytes.
- UDP max audio payload size: `1200` bytes.
- Status access key: `audiostatus`.

Business `key` rules:

- Non-empty.
- Max length `128` UTF-8 bytes.
- Treat the key as an opaque exact-match password/channel string, matching NVDA Remote behavior.
- Do not trim, lowercase, normalize, or restrict symbols, spaces, or Unicode characters.

TCP control behavior:

- Client sends one JSON line ending in `\n` immediately after connect.
- Role is `publisher` or `subscriber`.
- Successful response includes `status`, `role`, `key`, `session_id`, `udp_port`, `tcp_heartbeat_interval_ms`, `udp_session_timeout_ms`, and `udp_audio_payload_max_bytes`.
- `session_id` is exactly 16 bytes serialized as 32 hexadecimal characters.
- Each successful session must keep its TCP control connection alive.
- TCP heartbeat JSON is `{"type":"heartbeat"}`.
- TCP control disconnect immediately invalidates the session and its UDP endpoint.

UDP packet layout:

- Magic: `RAS1`.
- Version: `1`.
- Packet types: `0x01 register`, `0x02 register_ack`, `0x03 heartbeat`, `0x04 audio_data`.
- `session_id` is 16 raw bytes in UDP packets.
- `audio_data` metadata uses big-endian `u64` sequence and big-endian `u64` timestamp in milliseconds.
- Publisher audio must come from the registered UDP endpoint for the publisher session.
- Subscriber sessions must never be accepted as audio publishers.
- Forwarded audio keeps `sequence`, `timestamp_ms`, and payload unchanged, but replaces `session_id` with the target subscriber session id.

## Repository Layout

- `src/config.rs`: CLI arguments, defaults, protocol limits.
- `src/protocol.rs`: JSON and UDP encode/decode helpers.
- `src/state.rs`: session registry, stream state, counters, UDP endpoint validation.
- `src/server.rs`: TCP control server, UDP server, status server, dispatch workers, integration tests.
- `src/net.rs`: UDP socket binding and buffer sizing.
- `src/main.rs`: runtime entry point and logging setup.
- `src/bin/NVDARemoteAudioServer_load_test.rs`: real TCP/UDP load test tool.
- `deploy/systemd/NVDARemoteAudioServer.service`: Linux systemd service template.
- `.github/workflows/release.yml`: tag-triggered release pipeline.

Do not commit `target/`, `dist/`, local logs, captures, temporary stress-test outputs, or manually built binaries. Release binaries belong in GitHub Releases.

## Required Validation

Before committing code changes, run:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

For protocol or networking changes, also run a real local TCP/UDP load test:

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --publishers=20 --subscribers-per-publisher=20 --packets-per-publisher=200 --payload-bytes=1200
```

If a validation command cannot be run, state exactly which command was skipped and why.

## Release Contract

Releases are created by GitHub Actions when a tag is pushed.

Supported tag styles:

- `0.1`
- `v0.1.0`

Recommended release steps:

```bash
git status
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
git tag -a 0.1 -m "Release 0.1"
git push origin 0.1
```

The CI workflow must:

- Run format, clippy, and tests.
- Build Linux amd64.
- Build Windows amd64.
- Package binaries with `README.md`, `README-ZHCN.md`, and `LICENSE`.
- Create a GitHub Release from the pushed tag.

Do not manually add release binaries to the repository.

## Coding Rules

- Keep the server small and predictable.
- Prefer explicit error handling over `unwrap` or `expect` in production paths.
- `unwrap` and `expect` are acceptable in tests when they make test intent clearer.
- Keep UDP parsing length checks before slicing.
- Do not hold locks across `.await`.
- Do not add blocking I/O inside async hot paths.
- Do not add codec, audio, RTP, retry, retransmission, or buffering features to the server without a protocol decision.
- Do not change defaults or packet layouts without updating tests and both README files.
- Do not weaken endpoint binding: UDP packets must match the registered source address and port.
- Keep status output line-oriented JSON.
- Keep log output useful for operations, but avoid high-volume per-packet logs unless they are error paths.

## Documentation Rules

Update both documentation files together:

- `README.md`
- `README-ZHCN.md`

Whenever behavior changes, document:

- CLI arguments.
- Default ports.
- Deployment steps.
- Status behavior.
- Load test usage.
- Release behavior if CI changes.

Keep wording practical and readable. The README should help an operator get the server running without needing to understand the whole codebase first.

## Security And Operations Notes

- The status key is fixed as `audiostatus`; do not expose the status port publicly unless firewall rules are intentional.
- Production Linux deployment currently runs as `root` in the provided systemd unit because that was a project requirement. If this changes, update systemd and docs together.
- Open both TCP and UDP for port `6838`.
- Open TCP `6839` only if status access is needed.
- The server is a normal console process on Windows, not a native Windows Service binary. Do not document direct `sc.exe create` service installation unless Windows Service support is implemented in code.

## Handoff Checklist

Before handing work back:

- `git status --short` is understood and reported.
- No `target/` or `dist/` directory is prepared for commit.
- README and README-ZHCN are in sync.
- CI workflow still triggers on tag push.
- The release tag flow is not broken.
- All validation results are reported honestly.
