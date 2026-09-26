# Agent maintenance guide

These rules apply to changes to code, documentation, CI, and deployment files in `NVDARemoteAudioServer`.

## Scope

This Rust audio relay handles TCP authentication and heartbeats, UDP endpoint registration by `session_id`, audio forwarding, a separate TCP status interface, and operational logs.

Each `(key, stream)` allows at most one publisher and multiple subscribers. The independent streams are `system_audio`, `voice_controlled_to_controller`, and `voice_controller_to_controlled`.

Keep audio capture, playback, encoding, decoding, resampling, mixing, retransmission, RTP/RTCP, packet order repair, and codec-specific work in clients.

## Protocol contract

Changes to this contract require corresponding updates to the README and API documents, tests, load test tool, and downstream clients.

| Setting | Value |
| --- | --- |
| Default TCP control / UDP data port | `6838` |
| Default TCP status port | `6839` |
| TCP handshake request limit | 4096 bytes |
| TCP control message / status request limit | 1024 bytes |
| Handshake timeout | 5000 ms |
| TCP control idle timeout | 15000 ms |
| UDP endpoint inactivity timeout | 15000 ms |
| UDP packet size limit | 1400 bytes |
| UDP audio payload limit | 1200 bytes |
| Status access key | `audiostatus` |

### Connection keys

`key` is an exact-match password/channel string, following NVDA Remote's key rules. It must be non-empty and at most 128 UTF-8 bytes. Do not trim, change case, normalize, or restrict printable spaces, symbols, or Unicode. Reject control characters because keys appear in logs and line-oriented operational tools.

### TCP control

- Clients send a JSON line ending in `\n` immediately after connecting.
- `role` is `publisher` or `subscriber`; `stream` is required and names one of the three supported streams.
- Success responses contain `status`, `message`, `role`, `key`, `stream`, `session_id`, `udp_port`, `tcp_heartbeat_interval_ms`, `udp_session_timeout_ms`, and `udp_audio_payload_max_bytes`.
- `session_id` is 16 bytes, serialized as 32 hexadecimal characters.
- Each session keeps its TCP control connection alive with `{"type":"heartbeat"}`.
- A control disconnect immediately invalidates the session and its UDP endpoint.

### UDP

- Magic: `RAS1`; version: `1`.
- Types: `0x01 register`, `0x02 register_ack`, `0x03 heartbeat`, `0x04 audio_data`.
- `session_id` uses 16 raw bytes.
- `audio_data` uses big-endian `u64` fields for `sequence` and `timestamp_ms` (milliseconds).
- UDP heartbeats and audio must match the registered source IP address and port. Only publishers may send audio.
- Forward only to active subscribers on the same `(key, stream)`. Preserve `sequence`, `timestamp_ms`, and `payload`; replace `session_id` with the recipient's session ID.

## Repository layout

| Path | Responsibility |
| --- | --- |
| `src/config.rs` | CLI arguments, defaults, and protocol limits |
| `src/protocol.rs` | JSON and UDP encoding, decoding, and validation |
| `src/state.rs` | Sessions, streams, counters, and UDP endpoint validation |
| `src/server.rs` | TCP control, UDP forwarding, status queries, dispatch workers, and integration tests |
| `src/net.rs` | UDP socket binding and buffer sizes |
| `src/main.rs` | Runtime entry point and logging |
| `src/bin/NVDARemoteAudioServer_load_test.rs` | TCP/UDP load test tool |
| `deploy/systemd/NVDARemoteAudioServer.service` | Linux systemd service template |
| `.github/workflows/release.yml` | Tag-triggered releases |

Do not commit `target/`, `dist/`, local logs, packet captures, temporary load test output, or manually built binaries. Publish binaries through GitHub Releases.

## Validation

Before committing code changes, run:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

For protocol or networking changes, also run the local TCP/UDP load test:

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --stream=system_audio --publishers=20 --subscribers-per-publisher=20 --packets-per-publisher=200 --payload-bytes=1200
```

If a command cannot be run, report the exact command and reason.

## Releases

GitHub Actions releases on tag push; supported formats include `0.1` and `v0.1.0`.

Check `git status`, run the validation commands above, and use an unused version tag. Example:

```bash
git tag -a 0.6 -m "Release 0.6"
git push origin 0.6
```

The workflow must:

- Run format checks, Clippy, and tests.
- Build Linux amd64 and Windows amd64 packages.
- Include `README.md`, `README-ZHCN.md`, `API.md`, `API-ZHCN.md`, and `LICENSE` with the binaries.
- Generate Chinese and English release notes covering the version summary, protocol compatibility, CI validation, artifacts, and commit summary.
- Create a GitHub Release from the pushed tag.

## Coding rules

- Keep the server small and its behavior predictable.
- Prefer explicit error handling over `unwrap` or `expect` in production. Both are acceptable in tests when they clarify intent.
- Check UDP lengths before slicing.
- Do not hold locks across `.await` or add blocking I/O to async hot paths.
- Retry or buffering features require a protocol decision before implementation.
- Keep status output as one JSON object per line.
- Keep logs useful for operations; avoid high-volume per-packet logs except on error paths.

## Documentation

Update the public documents together: `README.md`, `README-ZHCN.md`, `API.md`, and `API-ZHCN.md`. Keep `AGENTS.md` and `AGENTS-ZHCN.md` in sync as well.

Document changes to CLI arguments (including the load test stream selector), default ports, TCP/UDP/status behavior, deployment, load testing, and release CI.

README covers operation; API documents cover client integration. Verify claims against code. Keep languages equivalent, terms consistent, and wording natural. Avoid repetition and promotional language.

## Operations

- Allow TCP and UDP `6838`. Open status port TCP `6839` only as needed with intentional firewall rules; its key is fixed as `audiostatus`.
- The supplied Linux systemd service runs as `root`, as required by the project. Any change must update the service and documentation together.
- Windows builds are console programs. Do not document direct installation with `sc.exe create` unless Windows Service support is implemented.

## Handoff

- Review and report `git status --short`; do not stage `target/` or `dist/`.
- Confirm that all three English/Chinese document pairs agree.
- Confirm that CI still triggers on tag pushes and the release flow remains intact.
- Report validation results and any skipped checks accurately.
