# NVDARemoteAudioServer

[简体中文](README-ZHCN.md) · [API reference](API.md)

NVDARemoteAudioServer is a Rust relay server for low-latency remote audio. TCP handles authentication and session heartbeats; UDP carries audio packets. The server routes packets by `(key, stream)`, with at most one publisher and multiple subscribers per stream.

Clients handle audio capture, playback, and processing. The server does not implement RTP/RTCP, retransmit packets, or repair packet order.

## Streams and keys

| `stream` | Direction |
| --- | --- |
| `system_audio` | Controlled device's system audio → controller |
| `voice_controlled_to_controller` | Controlled device's microphone → controller |
| `voice_controller_to_controlled` | Controller's microphone → controlled device |

A channel's `key` is its shared password, matched exactly as in NVDA Remote. It must be non-empty, contain no control characters, and fit within 128 UTF-8 bytes. Spaces, symbols, and Unicode are allowed; the server does not trim, change case, or normalize keys.

The three streams under a key are independent. See the [API reference](API.md) for client integration.

## Ports and arguments

| Purpose | Transport | Default listener |
| --- | --- | --- |
| Control | TCP | `0.0.0.0:6838` |
| Audio data | UDP | `0.0.0.0:6838` |
| Status | TCP | `0.0.0.0:6839` |

| Argument | Description |
| --- | --- |
| `--port=6838` | TCP control and UDP data port |
| `--sport=6839` | TCP status port; must differ from the control port |
| `--log=/path/to/server.log` | Append logs to a file; omit to use standard output |
| `--help`, `-h` | Show usage |

Ports must be between 1 and 65535. Log directories are created automatically; log files are not rotated.

With default ports, allow TCP and UDP `6838` through the firewall. Restrict TCP `6839` to trusted hosts that need status access.

## Linux deployment

Install the latest amd64 release. Omit `sudo` when running as `root`.

```bash
mkdir -p /tmp/NVDARemoteAudioServer-release
cd /tmp/NVDARemoteAudioServer-release
curl -fL -o NVDARemoteAudioServer-linux-amd64.tar.gz https://github.com/haitun001/NVDARemoteAudioServer/releases/latest/download/NVDARemoteAudioServer-linux-amd64.tar.gz
tar -xzf NVDARemoteAudioServer-linux-amd64.tar.gz

sudo mkdir -p /home/app/NVDARemoteAudioServer/logs
sudo systemctl stop NVDARemoteAudioServer 2>/dev/null || true
sudo install -m 755 NVDARemoteAudioServer /home/app/NVDARemoteAudioServer/NVDARemoteAudioServer
sudo cp deploy/systemd/NVDARemoteAudioServer.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now NVDARemoteAudioServer
```

The service runs as `root` and restarts automatically. Check status and application logs:

```bash
systemctl status NVDARemoteAudioServer
sudo tail -f /home/app/NVDARemoteAudioServer/logs/NVDARemoteAudioServer.log
```

Use `sudo journalctl -u NVDARemoteAudioServer` to investigate service startup failures. For a locally built server, run the installation commands from the repository root, replacing the binary source with `target/release/NVDARemoteAudioServer`.

For UFW:

```bash
sudo ufw allow 6838/tcp
sudo ufw allow 6838/udp
```

## Windows deployment

Download `NVDARemoteAudioServer-windows-amd64.zip` from [GitHub Releases](https://github.com/haitun001/NVDARemoteAudioServer/releases). In PowerShell, extract it and start the server:

```powershell
Expand-Archive .\NVDARemoteAudioServer-windows-amd64.zip -DestinationPath C:\NVDARemoteAudioServer -Force
C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

For a local build, copy `target\release\NVDARemoteAudioServer.exe` to the same directory. Stop the running process before replacing the executable.

To start at boot, create a Task Scheduler task with an “At startup” trigger and this action:

```text
Program: C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe
Arguments: --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

This is a console program; `sc.exe create` cannot install it directly as a service. Use WinSW or NSSM if service management is needed.

## Status queries

Send one JSON line ending in `\n` to the status port (TCP `6839` by default):

```json
{"key":"audiostatus"}
```

The server returns one JSON line and closes the connection. Read through the newline; response size varies with the number of streams. The fixed access key is `audiostatus`, and responses include channel keys. See the [status API](API.md#status-api) for connection counts and UDP statistics.

## Building from source

### Linux

On Debian or Ubuntu, install the build tools and Rust:

```bash
sudo apt-get update
sudo apt-get install -y curl git build-essential
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

### Windows

Install [Rust](https://rustup.rs), Git, and Visual Studio 2022 Build Tools with the “Desktop development with C++” workload. If MSVC tools are unavailable, run `.\dev-env.cmd` from the repository directory and build in the command prompt it opens.

### Build

```bash
git clone https://github.com/haitun001/NVDARemoteAudioServer.git
cd NVDARemoteAudioServer
cargo build --release --bins
```

Both executables, `NVDARemoteAudioServer` and `NVDARemoteAudioServer_load_test`, are in `target/release`. Windows filenames end in `.exe`.

### Building Linux amd64 on Windows

Use the musl target to build a static Linux executable:

```powershell
rustup target add x86_64-unknown-linux-musl
$env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="rust-lld"
cargo build --release --target x86_64-unknown-linux-musl --bin NVDARemoteAudioServer
```

Copy `target\x86_64-unknown-linux-musl\release\NVDARemoteAudioServer` to the Linux host and follow the deployment steps above.

## Validation and load testing

Run the checks from the repository root:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Local load test (starts a server automatically):

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --stream=system_audio --publishers=20 --subscribers-per-publisher=20 --packets-per-publisher=200 --payload-bytes=1200
```

To test an existing server:

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --host=127.0.0.1 --port=6838 --sport=6839 --stream=system_audio --external-server
```

Use an isolated test server: the tool checks global status counters, so other traffic or earlier UDP errors can make validation fail. Both the control/data port and status port must be reachable.

| Option | Default | Description |
| --- | --- | --- |
| `--host` | `127.0.0.1` | Server address; must resolve to loopback unless `--external-server` is set |
| `--port` | `6838` | TCP control and UDP data port |
| `--sport` | `6839` | TCP status port |
| `--stream` | `system_audio` | One of the three streams listed above |
| `--publishers` | `20` | Publisher count, each using a separate key |
| `--subscribers-per-publisher` | `20` | Subscribers for each publisher |
| `--packets-per-publisher` | `200` | Packets sent by each publisher |
| `--payload-bytes` | `160` | Payload size, from 1 to 1200 bytes |
| `--packet-interval-ms` | `10` | Delay between packets; `0` sends without a delay |
| `--heartbeat-rounds` | `2` | Minimum wait rounds before collecting results; extends while publishers are sending |
| `--heartbeat-round-interval-ms` | `1000` | Wait round interval; does not change the TCP or UDP heartbeat interval |
| `--external-server` | Off | Connect to an existing server |
| `--help`, `-h` | — | Show usage |

The tool checks forwarded data and status counters; missing, duplicate, or invalid packets fail validation. The packaged load test executable accepts the same options.

## Releases

Pushing a version tag triggers format checks, Clippy, tests, and Linux amd64 / Windows amd64 builds. GitHub Actions then creates a [GitHub Release](https://github.com/haitun001/NVDARemoteAudioServer/releases) with Chinese and English release notes. Packages include the server, load test tool, bilingual README and API files, license, and systemd service file.

Tags support both `0.1` and `v0.1.0` formats. After validation, use an unused version number:

```bash
git tag -a 0.6 -m "Release 0.6"
git push origin 0.6
```

## License

[GNU General Public License v2.0 only](LICENSE) (`GPL-2.0-only`).
