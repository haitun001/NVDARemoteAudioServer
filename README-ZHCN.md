# NVDARemoteAudioServer

[English](README.md) · [API 文档](API-ZHCN.md)

NVDARemoteAudioServer 是用 Rust 编写的低延迟远程音频转发服务端。TCP 负责认证和会话心跳，UDP 传输音频数据。服务端按 `(key, stream)` 转发数据，每条流最多允许一个推流端，可有多个拉流端。

音频采集、播放和处理由客户端负责。服务端不实现 RTP/RTCP，不重传数据包或修复包顺序。

## 音频流与连接密钥

| `stream` | 方向 |
| --- | --- |
| `system_audio` | 被控端系统音频 → 控制端 |
| `voice_controlled_to_controller` | 被控端麦克风 → 控制端 |
| `voice_controller_to_controlled` | 控制端麦克风 → 被控端 |

`key` 是通道的共享密码，按 NVDA Remote 的规则原样匹配。它不能为空、不能包含控制字符，长度不超过 128 个 UTF-8 字节。允许空格、符号和 Unicode 字符；不去除空白、不转换大小写、不做 Unicode 归一化。

同一密钥下的三条流彼此独立。客户端接入说明见 [API 文档](API-ZHCN.md)。

## 端口与参数

| 用途 | 传输协议 | 默认监听地址 |
| --- | --- | --- |
| 控制 | TCP | `0.0.0.0:6838` |
| 音频数据 | UDP | `0.0.0.0:6838` |
| 状态查询 | TCP | `0.0.0.0:6839` |

| 参数 | 说明 |
| --- | --- |
| `--port=6838` | TCP 控制端口和 UDP 数据端口 |
| `--sport=6839` | TCP 状态端口，须与控制端口不同 |
| `--log=/path/to/server.log` | 向文件追加日志；省略时输出到标准输出 |
| `--help`, `-h` | 显示用法 |

端口范围为 1 至 65535。日志目录自动创建，日志文件不自动轮转。

使用默认端口时，防火墙须放行 TCP 和 UDP `6838`；TCP `6839` 仅向需要查询状态的可信主机开放。

## Linux 部署

安装最新 amd64 发布版。以 `root` 运行时可省略 `sudo`。

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

服务以 `root` 运行，并配置了自动重启。查看状态和应用日志：

```bash
systemctl status NVDARemoteAudioServer
sudo tail -f /home/app/NVDARemoteAudioServer/logs/NVDARemoteAudioServer.log
```

服务启动失败时，用 `sudo journalctl -u NVDARemoteAudioServer` 排查。部署本地构建的程序时，在仓库根目录执行上述安装命令，将二进制源路径换成 `target/release/NVDARemoteAudioServer`。

使用 UFW 时：

```bash
sudo ufw allow 6838/tcp
sudo ufw allow 6838/udp
```

## Windows 部署

从 [GitHub Releases](https://github.com/haitun001/NVDARemoteAudioServer/releases) 下载 `NVDARemoteAudioServer-windows-amd64.zip`，在 PowerShell 中解压并启动：

```powershell
Expand-Archive .\NVDARemoteAudioServer-windows-amd64.zip -DestinationPath C:\NVDARemoteAudioServer -Force
C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

本地构建时，将 `target\release\NVDARemoteAudioServer.exe` 复制到同一目录。替换程序前先停止正在运行的进程。

需要开机启动时，在“任务计划程序”中新建任务，触发器选择“启动时”，操作填写：

```text
程序: C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe
参数: --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

程序以控制台进程运行，不能直接用 `sc.exe create` 安装为服务；如需按服务管理，可使用 WinSW 或 NSSM。

## 状态查询

向状态端口（默认 TCP `6839`）发送一行以 `\n` 结尾的 JSON：

```json
{"key":"audiostatus"}
```

服务端返回一行 JSON 后关闭连接。响应大小随流数量变化，应读取到换行符为止。访问密钥固定为 `audiostatus`，响应包含连接密钥。连接数和 UDP 统计说明见[状态 API](API-ZHCN.md#状态-api)。

## 从源码构建

### Linux

在 Debian 或 Ubuntu 上安装构建工具和 Rust：

```bash
sudo apt-get update
sudo apt-get install -y curl git build-essential
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

### Windows

安装 [Rust](https://rustup.rs)、Git 和 Visual Studio 2022 Build Tools，选择“使用 C++ 的桌面开发”工作负载。若找不到 MSVC 工具，在仓库目录运行 `.\dev-env.cmd`，再到打开的命令提示符中构建。

### 构建

```bash
git clone https://github.com/haitun001/NVDARemoteAudioServer.git
cd NVDARemoteAudioServer
cargo build --release --bins
```

服务端 `NVDARemoteAudioServer` 和压测工具 `NVDARemoteAudioServer_load_test` 位于 `target/release`，Windows 文件带 `.exe` 后缀。

### 在 Windows 上构建 Linux amd64

使用 musl 目标构建 Linux 静态可执行文件：

```powershell
rustup target add x86_64-unknown-linux-musl
$env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="rust-lld"
cargo build --release --target x86_64-unknown-linux-musl --bin NVDARemoteAudioServer
```

将 `target\x86_64-unknown-linux-musl\release\NVDARemoteAudioServer` 复制到 Linux 主机，按上述部署步骤安装。

## 检查与压测

在仓库根目录运行检查：

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

本地压测（自动启动服务）：

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --stream=system_audio --publishers=20 --subscribers-per-publisher=20 --packets-per-publisher=200 --payload-bytes=1200
```

测试已有服务：

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --host=127.0.0.1 --port=6838 --sport=6839 --stream=system_audio --external-server
```

请使用独立的测试服务：工具会校验全局状态计数，其他流量或此前的 UDP 错误可能导致校验失败。控制、数据和状态端口均须可访问。

| 参数 | 默认值 | 说明 |
| --- | --- | --- |
| `--host` | `127.0.0.1` | 服务端地址；未指定 `--external-server` 时须解析为回环地址 |
| `--port` | `6838` | TCP 控制端口和 UDP 数据端口 |
| `--sport` | `6839` | TCP 状态端口 |
| `--stream` | `system_audio` | 上文列出的三条流之一 |
| `--publishers` | `20` | 推流端数量，各自使用不同密钥 |
| `--subscribers-per-publisher` | `20` | 每个推流端对应的拉流端数量 |
| `--packets-per-publisher` | `200` | 每个推流端发送的包数 |
| `--payload-bytes` | `160` | 载荷大小，范围为 1 至 1200 字节 |
| `--packet-interval-ms` | `10` | 发包间隔；`0` 表示不等待 |
| `--heartbeat-rounds` | `2` | 收集结果前的最少等待轮数；推流未结束时会自动延长 |
| `--heartbeat-round-interval-ms` | `1000` | 每轮等待时长，不改变 TCP 或 UDP 心跳间隔 |
| `--external-server` | 关闭 | 连接已有服务 |
| `--help`, `-h` | — | 显示用法 |

工具校验转发数据和状态计数，缺包、重复包或无效包均视为失败。发布包内的压测工具接受相同参数。

## 发布

推送版本标签后，GitHub Actions 运行格式检查、Clippy 和测试，构建 Linux amd64 与 Windows amd64 发布包，再创建附有中英文发布说明的 [GitHub Release](https://github.com/haitun001/NVDARemoteAudioServer/releases)。包内包含服务端、压测工具、中英文 README 和 API 文档、许可证及 systemd 服务文件。

支持 `0.1` 和 `v0.1.0` 两种标签格式。验证完成后，使用尚未发布的版本号：

```bash
git tag -a 0.6 -m "Release 0.6"
git push origin 0.6
```

## 许可证

[GNU General Public License v2.0 only](LICENSE)（`GPL-2.0-only`）。
