# NVDARemoteAudioServer

NVDARemoteAudioServer is a small Rust relay server for low-latency remote audio links. It keeps the server side intentionally simple: TCP is used for authentication and heartbeats, UDP is used for audio packets, and the server only routes packets by key.

The server does not capture, encode, decode, mix, resample, retransmit, or repair audio. Clients own all audio work. This keeps the server predictable under load and easier to operate for a long time.

## What It Does

- Accepts one publisher per key.
- Accepts many subscribers for the same key.
- Keeps each client alive through a TCP control session.
- Registers each UDP endpoint before audio can flow.
- Forwards UDP audio packets from the publisher to active subscribers.
- Rewrites the forwarded UDP packet session id to the subscriber session id.
- Exposes a status TCP port protected by the fixed status key `audiostatus`.
- Writes logs to stdout or to the file passed with `--log=...`.

## Stream Key / Password

The business `key` is handled like NVDA Remote handles its relay key: it is an opaque password/channel string and authentication is an exact string match. The server requires it to be non-empty and at most 128 UTF-8 bytes. It does not trim, lowercase, normalize, or reject printable spaces, symbols, or Unicode characters. Control characters are rejected because they can pollute logs and line-oriented operational tools.

## Ports And Arguments

Defaults:

- TCP control: `0.0.0.0:6838`
- UDP audio/data: `0.0.0.0:6838`
- TCP status: `0.0.0.0:6839`

Run options:

```bash
NVDARemoteAudioServer --port=6838 --sport=6839 --log=/home/app/NVDARemoteAudioServer/logs/NVDARemoteAudioServer.log
```

- `--port=6838` sets both TCP control and UDP data port.
- `--sport=6839` sets the TCP status port.
- `--log=/path/to/file.log` writes logs to a file. Without it, logs go to stdout.

## Build On Linux

Prepare a Debian-family build machine first:

```bash
sudo apt-get update
sudo apt-get install -y curl git build-essential
```

If you are already logged in as `root`, run the same commands without `sudo`. If you are not `root`, keep `sudo`.

Install Rust first:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

Build the release binary:

```bash
git clone https://github.com/haitun001/NVDARemoteAudioServer.git
cd NVDARemoteAudioServer
cargo build --release --bin NVDARemoteAudioServer
```

The binary will be here:

```bash
target/release/NVDARemoteAudioServer
```

Run tests:

```bash
cargo test
```

Build the load test tool:

```bash
cargo build --release --bin NVDARemoteAudioServer_load_test
```

## Build On Windows

Install these pieces:

- Rust from https://rustup.rs
- Visual Studio 2022 Build Tools with the C++ workload

Then open PowerShell in this directory and run:

```powershell
cargo build --release --bin NVDARemoteAudioServer
cargo test
```

The Windows binary will be here:

```powershell
target\release\NVDARemoteAudioServer.exe
```

If your shell does not have the MSVC build tools loaded, run:

```cmd
dev-env.cmd
```

Then build again from that developer environment.

## Build Linux amd64 From Windows

This project currently has no C/C++ native dependency, so Windows can build a static Linux amd64 binary with Rust's musl target:

```powershell
rustup target add x86_64-unknown-linux-musl
$env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="rust-lld"
cargo build --release --target x86_64-unknown-linux-musl --bin NVDARemoteAudioServer
```

The binary will be here:

```powershell
target\x86_64-unknown-linux-musl\release\NVDARemoteAudioServer
```

That file is an x86-64 Linux executable and is suitable for Debian-family amd64 machines. Copy it to the Linux deployment directory and run `chmod 755` after copying.

WSL Debian is still a good path when you want the build environment to look almost the same as the deployment machine:

```powershell
wsl --install -d Debian
```

Inside Debian:

```bash
cd /mnt/c/Users/Administrator/Documents/codex/RemoteAudio/NVDARemoteAudioServer
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
cargo build --release --bin NVDARemoteAudioServer
```

The Linux binary will be:

```bash
target/release/NVDARemoteAudioServer
```

## Linux Deployment

You have two practical ways to deploy on Linux. If you just want to run the server, use the GitHub Releases binary. If you want to audit or rebuild everything yourself, build from source first and then use the same deployment layout.

The Linux commands below use `sudo`. If you are already logged in as `root`, run the same commands without `sudo`. If you are not `root`, keep `sudo`.

### Linux: Deploy From GitHub Releases

Open the latest release page:

```text
https://github.com/haitun001/NVDARemoteAudioServer/releases
```

Download `NVDARemoteAudioServer-linux-amd64.tar.gz` to the server, then unpack and install it:

```bash
mkdir -p /tmp/NVDARemoteAudioServer-download
cd /tmp/NVDARemoteAudioServer-download
curl -L -o NVDARemoteAudioServer-linux-amd64.tar.gz https://github.com/haitun001/NVDARemoteAudioServer/releases/latest/download/NVDARemoteAudioServer-linux-amd64.tar.gz

mkdir -p /tmp/NVDARemoteAudioServer-release
tar -xzf NVDARemoteAudioServer-linux-amd64.tar.gz -C /tmp/NVDARemoteAudioServer-release

sudo mkdir -p /home/app/NVDARemoteAudioServer/logs
sudo systemctl stop NVDARemoteAudioServer 2>/dev/null || true
sudo cp /tmp/NVDARemoteAudioServer-release/NVDARemoteAudioServer /home/app/NVDARemoteAudioServer/
sudo chmod 755 /home/app/NVDARemoteAudioServer/NVDARemoteAudioServer
```

The release archive also includes `README.md`, `README-ZHCN.md`, `LICENSE`, and `deploy/systemd/NVDARemoteAudioServer.service`, so you can use the same documentation and systemd unit after unpacking it.

If you downloaded the archive on another machine first, copy it to Linux with `scp`, SFTP, or your normal deployment tool. The binary in this package is built by GitHub Actions from the tagged source code.

### Linux: Deploy A Locally Built Binary

Recommended directory layout:

```bash
/home/app/NVDARemoteAudioServer/
  NVDARemoteAudioServer
  logs/
```

Copy the binary:

```bash
sudo mkdir -p /home/app/NVDARemoteAudioServer/logs
sudo systemctl stop NVDARemoteAudioServer 2>/dev/null || true
sudo cp target/release/NVDARemoteAudioServer /home/app/NVDARemoteAudioServer/
sudo chmod 755 /home/app/NVDARemoteAudioServer/NVDARemoteAudioServer
```

If you built the Linux amd64 musl binary from Windows, use this source path instead:

```bash
target/x86_64-unknown-linux-musl/release/NVDARemoteAudioServer
```

### Linux: Install systemd

Install the systemd unit:

```bash
sudo cp deploy/systemd/NVDARemoteAudioServer.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now NVDARemoteAudioServer
```

Check it:

```bash
systemctl status NVDARemoteAudioServer
journalctl -u NVDARemoteAudioServer -f
```

Open firewall ports if needed:

```bash
sudo ufw allow 6838/tcp
sudo ufw allow 6838/udp
sudo ufw allow 6839/tcp
```

## Windows Deployment

You can deploy Windows builds from GitHub Releases or from a local source build. The executable is a normal console server, not a native Windows Service binary.

### Windows: Deploy From GitHub Releases

Open the latest release page:

```text
https://github.com/haitun001/NVDARemoteAudioServer/releases
```

Download `NVDARemoteAudioServer-windows-amd64.zip`, extract it, and copy the executable into a fixed directory:

```powershell
New-Item -ItemType Directory -Force C:\NVDARemoteAudioServer\logs
Expand-Archive .\NVDARemoteAudioServer-windows-amd64.zip -DestinationPath C:\NVDARemoteAudioServer\release -Force
Copy-Item C:\NVDARemoteAudioServer\release\NVDARemoteAudioServer.exe C:\NVDARemoteAudioServer\
```

Start it manually:

```powershell
C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

### Windows: Deploy A Locally Built Binary

For a simple foreground run:

```powershell
.\target\release\NVDARemoteAudioServer.exe --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

For a permanent Windows deployment, put the executable in a fixed directory:

```powershell
New-Item -ItemType Directory -Force C:\NVDARemoteAudioServer\logs
Copy-Item .\target\release\NVDARemoteAudioServer.exe C:\NVDARemoteAudioServer\
```

### Windows: Startup Notes

This program is a normal console server, not a native Windows Service binary. Do not install it directly with `sc.exe create`, because Windows services need a Service Control Dispatcher. For startup at boot, use Task Scheduler or wrap it with a tool such as WinSW or NSSM.

Task Scheduler is the simplest built-in option. Create a task that runs at startup and uses this action:

```text
Program: C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe
Arguments: --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

You can also start it manually from PowerShell:

```powershell
Start-Process C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe -ArgumentList "--port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log"
```

Open Windows Firewall for TCP and UDP port `6838`, plus TCP port `6839` if status access is needed.

## Status Check

Send one JSON line to the status port:

```json
{"key":"audiostatus"}
```

The response is one JSON line. Larger deployments can produce a large status snapshot, so read until the newline instead of using a tiny fixed buffer.

## Load Test

Local test with 20 publishers and 20 subscribers per publisher:

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --publishers=20 --subscribers-per-publisher=20
```

Against an already running server:

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --host=127.0.0.1 --port=6838 --sport=6839 --external-server
```

The tool uses real TCP sessions, real UDP register/heartbeat packets, real UDP audio packets, and validates that subscribers receive the expected payloads.

## License

NVDARemoteAudioServer is licensed under the GNU General Public License version 2 only. See `LICENSE` for the full license text.

## Release CI

GitHub Actions creates a release when a version tag is pushed. Both numeric tags such as `0.1` and conventional tags such as `v0.1.0` are supported.

```bash
git tag -a 0.1 -m "Release 0.1"
git push origin 0.1
```
