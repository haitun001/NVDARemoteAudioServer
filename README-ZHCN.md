# NVDARemoteAudioServer

NVDARemoteAudioServer 是一个用 Rust 写的低延迟远程音频转发服务端。它的职责很克制：TCP 负责认证和心跳，UDP 负责音频数据，服务端只按 key 做路由和分发。

服务端不采集音频，不编码，不解码，不混音，不转码，也不做重传和排序修复。音频处理交给客户端，服务端就专心把链路跑稳。

## 它负责什么

- 同一个 key 只允许一个推流端。
- 同一个 key 可以有多个拉流端。
- 每个客户端都要保持 TCP 控制连接。
- UDP 端点必须先 register，收到 register_ack 后才算注册成功。
- 推流端发来的 UDP 音频包会分发给已注册并且仍活跃的拉流端。
- 转发给拉流端时，会把 UDP 包里的 session_id 替换成拉流端自己的 session_id。
- 状态查询走独立 TCP 端口，访问 key 固定为 `audiostatus`。
- 日志可以写到 stdout，也可以通过 `--log=...` 写到文件。

## 推流 key / 密码

业务 `key` 按照 NVDA Remote 的中继 key 行为处理：它是一个不透明的密码/通道字符串，认证时只做原样精确匹配。服务端只要求它非空、最长 128 个 UTF-8 字节，不会自动去掉空格，不会转小写，不会做 Unicode 归一化，也不会拒绝可打印的空格、符号或中文等 Unicode 字符。控制字符会被拒绝，因为它们会污染日志和按行读取的运维工具。

## 端口和启动参数

默认监听：

- TCP 控制端口：`0.0.0.0:6838`
- UDP 数据端口：`0.0.0.0:6838`
- TCP 状态端口：`0.0.0.0:6839`

启动示例：

```bash
NVDARemoteAudioServer --port=6838 --sport=6839 --log=/home/app/NVDARemoteAudioServer/logs/NVDARemoteAudioServer.log
```

- `--port=6838` 同时设置 TCP 控制端口和 UDP 数据端口。
- `--sport=6839` 设置 TCP 状态端口。
- `--log=/path/to/file.log` 把日志写到文件。不传这个参数时，日志输出到 stdout。

## Linux 从源码构建

先准备 Debian 系列构建环境：

```bash
sudo apt-get update
sudo apt-get install -y curl git
```

如果已经用 `root` 登录，可以去掉 `sudo` 直接执行同样的命令。如果不是 `root` 用户，请保留 `sudo`。

先安装 Rust：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

构建发布版：

```bash
git clone https://github.com/haitun001/NVDARemoteAudioServer.git
cd NVDARemoteAudioServer
cargo build --release --bin NVDARemoteAudioServer
```

构建后的二进制在这里：

```bash
target/release/NVDARemoteAudioServer
```

运行测试：

```bash
cargo test
```

构建压测工具：

```bash
cargo build --release --bin NVDARemoteAudioServer_load_test
```

## Windows 从源码构建

需要准备：

- Rust：https://rustup.rs
- Visual Studio 2022 Build Tools，并安装 C++ workload

在当前目录打开 PowerShell：

```powershell
cargo build --release --bin NVDARemoteAudioServer
cargo test
```

Windows 版二进制在这里：

```powershell
target\release\NVDARemoteAudioServer.exe
```

如果当前 shell 没有加载 MSVC 编译环境，可以先运行：

```cmd
dev-env.cmd
```

然后再执行 cargo build。

## Windows 构建 Linux amd64

当前工程没有 C/C++ 原生依赖，所以可以在 Windows 里直接用 Rust 的 musl target 构建 Linux amd64 静态二进制：

```powershell
rustup target add x86_64-unknown-linux-musl
$env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="rust-lld"
cargo build --release --target x86_64-unknown-linux-musl --bin NVDARemoteAudioServer
```

构建结果在这里：

```powershell
target\x86_64-unknown-linux-musl\release\NVDARemoteAudioServer
```

这个文件是 x86-64 Linux 可执行文件，适合 Debian 系列 amd64 机器。复制到 Linux 部署目录后，记得执行 `chmod 755`。

如果希望构建环境尽量贴近真实部署机器，也可以用 WSL Debian：

```powershell
wsl --install -d Debian
```

进入 Debian 后：

```bash
cd /mnt/c/Users/Administrator/Documents/codex/RemoteAudio/NVDARemoteAudioServer
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
cargo build --release --bin NVDARemoteAudioServer
```

Linux amd64 二进制会在：

```bash
target/release/NVDARemoteAudioServer
```

## Linux 部署

Linux 上有两种常用部署方式。只想直接运行服务时，建议从 GitHub Releases 下载二进制。希望自己审计或重建时，可以先从源码构建，再按同样的目录部署。

下面的 Linux 命令使用了 `sudo`。如果已经用 `root` 登录，可以去掉 `sudo` 直接执行同样的命令。如果不是 `root` 用户，请保留 `sudo`。

### Linux：从 GitHub Releases 部署

打开最新 Release 页面：

```text
https://github.com/haitun001/NVDARemoteAudioServer/releases
```

下载 `NVDARemoteAudioServer-linux-amd64.tar.gz` 到服务器，然后解压并安装：

```bash
mkdir -p /tmp/NVDARemoteAudioServer-download
cd /tmp/NVDARemoteAudioServer-download
curl -L -o NVDARemoteAudioServer-linux-amd64.tar.gz https://github.com/haitun001/NVDARemoteAudioServer/releases/latest/download/NVDARemoteAudioServer-linux-amd64.tar.gz

mkdir -p /tmp/NVDARemoteAudioServer-release
tar -xzf NVDARemoteAudioServer-linux-amd64.tar.gz -C /tmp/NVDARemoteAudioServer-release

sudo mkdir -p /home/app/NVDARemoteAudioServer/logs
sudo cp /tmp/NVDARemoteAudioServer-release/NVDARemoteAudioServer /home/app/NVDARemoteAudioServer/
sudo chmod 755 /home/app/NVDARemoteAudioServer/NVDARemoteAudioServer
```

发布压缩包里也包含 `README.md`、`README-ZHCN.md`、`LICENSE` 和 `deploy/systemd/NVDARemoteAudioServer.service`，解压后可以直接使用同一份说明和 systemd 服务文件。

如果你是在别的机器上下载的压缩包，可以用 `scp`、SFTP 或平时使用的部署工具复制到 Linux。这个压缩包里的二进制由 GitHub Actions 根据对应标签的源码构建。

### Linux：部署本地构建的二进制

推荐目录：

```bash
/home/app/NVDARemoteAudioServer/
  NVDARemoteAudioServer
  logs/
```

复制二进制：

```bash
sudo mkdir -p /home/app/NVDARemoteAudioServer/logs
sudo cp target/release/NVDARemoteAudioServer /home/app/NVDARemoteAudioServer/
sudo chmod 755 /home/app/NVDARemoteAudioServer/NVDARemoteAudioServer
```

如果是在 Windows 上构建的 Linux amd64 musl 二进制，源文件路径换成：

```bash
target/x86_64-unknown-linux-musl/release/NVDARemoteAudioServer
```

### Linux：安装 systemd 服务

安装 systemd 服务：

```bash
sudo cp deploy/systemd/NVDARemoteAudioServer.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now NVDARemoteAudioServer
```

查看状态和日志：

```bash
systemctl status NVDARemoteAudioServer
journalctl -u NVDARemoteAudioServer -f
```

如果系统开了防火墙，放行端口：

```bash
sudo ufw allow 6838/tcp
sudo ufw allow 6838/udp
sudo ufw allow 6839/tcp
```

## Windows 部署

Windows 可以从 GitHub Releases 下载，也可以用本地源码构建出的 exe。注意：这个程序是普通控制台服务端，不是原生 Windows Service 程序。

### Windows：从 GitHub Releases 部署

打开最新 Release 页面：

```text
https://github.com/haitun001/NVDARemoteAudioServer/releases
```

下载 `NVDARemoteAudioServer-windows-amd64.zip`，解压后把 exe 放到固定目录：

```powershell
New-Item -ItemType Directory -Force C:\NVDARemoteAudioServer\logs
Expand-Archive .\NVDARemoteAudioServer-windows-amd64.zip -DestinationPath C:\NVDARemoteAudioServer\release -Force
Copy-Item C:\NVDARemoteAudioServer\release\NVDARemoteAudioServer.exe C:\NVDARemoteAudioServer\
```

手动启动：

```powershell
C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

### Windows：部署本地构建的二进制

前台直接运行：

```powershell
.\target\release\NVDARemoteAudioServer.exe --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

如果要长期放在 Windows 上运行，先把程序放到固定目录：

```powershell
New-Item -ItemType Directory -Force C:\NVDARemoteAudioServer\logs
Copy-Item .\target\release\NVDARemoteAudioServer.exe C:\NVDARemoteAudioServer\
```

### Windows：开机启动说明

当前程序是普通控制台服务端，不是原生 Windows Service 程序。不要直接用 `sc.exe create` 安装，因为 Windows Service 需要实现 Service Control Dispatcher，普通 exe 直接注册通常会启动失败。

如果希望开机自动启动，推荐用“任务计划程序”，或者用 WinSW/NSSM 这类包装工具把普通 exe 包成服务。任务计划程序里可以这样填写：

```text
程序: C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe
参数: --port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log
```

也可以用 PowerShell 手动放到后台运行：

```powershell
Start-Process C:\NVDARemoteAudioServer\NVDARemoteAudioServer.exe -ArgumentList "--port=6838 --sport=6839 --log=C:\NVDARemoteAudioServer\logs\server.log"
```

Windows 防火墙需要放行 TCP/UDP `6838`。如果要从外部访问状态接口，再放行 TCP `6839`。

## 状态查询

向状态端口发送一行 JSON：

```json
{"key":"audiostatus"}
```

响应也是一行 JSON。连接很多时状态快照会比较大，客户端要按整行读取，不要只读一个很小的固定缓冲区。

## 压测工具

本地启动服务并测试 20 个推流、每个推流 20 个拉流：

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --publishers=20 --subscribers-per-publisher=20
```

测试已经运行中的服务：

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --host=127.0.0.1 --port=6838 --sport=6839 --external-server
```

这个工具会真的建立 TCP 控制连接，真的发送 UDP register、heartbeat 和 audio_data，也会校验拉流端收到的 payload 是否符合预期。

## 许可证

NVDARemoteAudioServer 使用 GNU General Public License version 2 only 许可证发布。完整许可证文本见 `LICENSE`。

## Release CI

推送版本标签时，GitHub Actions 会自动创建 Release。支持 `0.1` 这种数字标签，也支持 `v0.1.0` 这种常见标签。

```bash
git tag -a 0.1 -m "Release 0.1"
git push origin 0.1
```
