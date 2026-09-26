# 智能体维护指南

本指南适用于 `NVDARemoteAudioServer` 的代码、文档、CI 和部署文件变更。

## 项目范围

本项目是 Rust 音频转发服务端，负责 TCP 认证与心跳、按 `session_id` 注册 UDP 端点、转发音频、提供独立 TCP 状态接口及记录运行日志。

每个 `(key, stream)` 最多允许一个推流端，可有多个拉流端。支持 `system_audio`、`voice_controlled_to_controller` 和 `voice_controller_to_controlled` 三条独立音频流。

音频采集、播放、编解码、重采样、混音、重传、RTP/RTCP、包顺序修复及编解码器相关工作须留在客户端。

## 协议约定

修改以下约定时，须同步更新 README 和 API 文档、测试、压测工具及下游客户端。

| 配置 | 值 |
| --- | --- |
| 默认 TCP 控制／UDP 数据端口 | `6838` |
| 默认 TCP 状态端口 | `6839` |
| TCP 握手请求上限 | 4096 字节 |
| TCP 控制消息／状态请求上限 | 1024 字节 |
| 握手超时 | 5000 ms |
| TCP 控制空闲超时 | 15000 ms |
| UDP 端点空闲超时 | 15000 ms |
| UDP 包大小上限 | 1400 字节 |
| UDP 音频载荷上限 | 1200 字节 |
| 状态访问密钥 | `audiostatus` |

### 连接密钥

`key` 是原样匹配的密码／通道字符串，遵循 NVDA Remote 的密钥规则。它须非空，且不超过 128 个 UTF-8 字节。不得去除空白、转换大小写、做 Unicode 归一化，或限制可打印的空格、符号和 Unicode 字符。密钥会出现在日志和按行处理的运维工具中，因此须拒绝控制字符。

### TCP 控制

- 客户端连接后立即发送一行以 `\n` 结尾的 JSON。
- `role` 为 `publisher`（推流端）或 `subscriber`（拉流端）；`stream` 必填，取值为三条支持的流之一。
- 成功响应包含 `status`、`message`、`role`、`key`、`stream`、`session_id`、`udp_port`、`tcp_heartbeat_interval_ms`、`udp_session_timeout_ms` 和 `udp_audio_payload_max_bytes`。
- `session_id` 为 16 字节，序列化为 32 个十六进制字符。
- 每个会话通过 `{"type":"heartbeat"}` 维持自己的 TCP 控制连接。
- 控制连接断开后，对应会话和 UDP 端点立即失效。

### UDP

- 标识为 `RAS1`，版本为 `1`。
- 类型为 `0x01 register`、`0x02 register_ack`、`0x03 heartbeat`、`0x04 audio_data`。
- `session_id` 使用 16 字节原始值。
- `audio_data` 的 `sequence` 和 `timestamp_ms` 使用大端序 `u64`，时间戳单位为毫秒。
- UDP 心跳和音频须匹配已注册的源 IP 地址和端口；仅推流端可发送音频。
- 仅向同一 `(key, stream)` 下的活跃拉流端转发。保留 `sequence`、`timestamp_ms` 和 `payload`，将 `session_id` 替换为接收方的会话 ID。

## 仓库结构

| 路径 | 职责 |
| --- | --- |
| `src/config.rs` | 命令行参数、默认值和协议限制 |
| `src/protocol.rs` | JSON 和 UDP 编解码与校验 |
| `src/state.rs` | 会话、音频流、计数器和 UDP 端点校验 |
| `src/server.rs` | TCP 控制、UDP 转发、状态查询、分发任务和集成测试 |
| `src/net.rs` | UDP 套接字绑定和缓冲区大小 |
| `src/main.rs` | 运行入口和日志初始化 |
| `src/bin/NVDARemoteAudioServer_load_test.rs` | TCP/UDP 压测工具 |
| `deploy/systemd/NVDARemoteAudioServer.service` | Linux systemd 服务模板 |
| `.github/workflows/release.yml` | 标签触发的发布流程 |

不要提交 `target/`、`dist/`、本地日志、抓包文件、临时压测输出或手动构建的二进制。二进制通过 GitHub Releases 发布。

## 验证

提交代码变更前运行：

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

修改协议或网络逻辑后，还须运行本地 TCP/UDP 压测：

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --stream=system_audio --publishers=20 --subscribers-per-publisher=20 --packets-per-publisher=200 --payload-bytes=1200
```

命令无法运行时，须报告完整命令及原因。

## 发布

推送标签触发 GitHub Actions 发布，支持 `0.1` 和 `v0.1.0` 等格式。

先检查 `git status` 并运行上述验证命令，再使用尚未发布的版本号创建标签。例如：

```bash
git tag -a 0.6 -m "Release 0.6"
git push origin 0.6
```

发布流程须：

- 运行格式检查、Clippy 和测试。
- 构建 Linux amd64 和 Windows amd64 发布包。
- 随二进制打包 `README.md`、`README-ZHCN.md`、`API.md`、`API-ZHCN.md` 和 `LICENSE`。
- 生成中英文发布说明，包含版本摘要、协议兼容说明、CI 验证、附件和提交摘要。
- 根据推送的标签创建 GitHub Release。

## 编码规则

- 保持服务端简洁、行为明确。
- 生产代码优先显式处理错误，避免 `unwrap` 或 `expect`；测试中可在有助于表达意图时使用。
- UDP 解析须先检查长度，再切片。
- 不得持锁跨越 `.await`，也不得在异步高频路径中加入阻塞 I/O。
- 增加重试或缓冲功能前，须先确定协议要求。
- 状态输出保持每行一个 JSON 对象。
- 日志须有助于运维排查；除错误情况外，避免高频逐包日志。

## 文档

公开文档须同步更新：`README.md`、`README-ZHCN.md`、`API.md` 和 `API-ZHCN.md`。`AGENTS.md` 与 `AGENTS-ZHCN.md` 也须保持一致。

命令行参数（含压测流选择参数）、默认端口、TCP/UDP/状态行为、部署、压测或发布流程变化时，更新对应说明。

README 说明运行维护，API 文档说明客户端接入。以代码核实事实，中英文含义一致、术语统一、表达自然，避免重复和宣传性表述。

## 运维

- 放行 TCP 和 UDP `6838`。需要状态访问时，按明确的防火墙规则开放 TCP `6839`，其密钥固定为 `audiostatus`。
- 随附 Linux systemd 服务按项目要求以 `root` 运行。若调整运行用户，须同步修改服务文件和文档。
- Windows 构建为控制台程序。实现 Windows 服务支持之前，不得说明可直接用 `sc.exe create` 安装。

## 交付

- 检查并报告 `git status --short`，不得暂存 `target/` 或 `dist/`。
- 确认三组中英文文档含义一致。
- 确认 CI 仍由标签推送触发，发布流程完整。
- 如实报告验证结果和跳过的检查。
