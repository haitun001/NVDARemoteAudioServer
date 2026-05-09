# 智能体维护指南

本文定义智能体维护 `NVDARemoteAudioServer` 时必须遵守的契约。在修改代码、文档、CI 或部署文件前，请先阅读并遵守这里的要求。

## 项目目标

`NVDARemoteAudioServer` 是一个用 Rust 编写的低延迟远程音频传输中继服务端。

服务端职责刻意保持收敛：

- 通过 TCP 控制会话认证客户端。
- 保持 TCP 控制心跳。
- 按业务 `key` 路由数据流。
- 保证同一个 `key` 只允许一个推流端。
- 允许同一个 `key` 有多个拉流端。
- 按 `session_id` 注册 UDP 端点。
- 将一个推流端的 UDP 音频包转发给活跃拉流端。
- 在独立 TCP 状态端口暴露状态统计。
- 写出运行日志。

服务端不得执行音频采集、播放、编码、解码、重采样、混音、重传、RTP/RTCP、包排序修复或任何 codec 相关工作。这些工作属于客户端。

## 稳定协议契约

除非 README、测试、压测工具和下游客户端一起更新，否则不要修改以下契约。

- 默认 TCP 控制端口：`6838`。
- 默认 UDP 数据端口：`6838`。
- 默认 TCP 状态端口：`6839`。
- TCP 握手最大请求大小：`4096` 字节。
- TCP 控制消息最大请求大小：`1024` 字节。
- TCP 状态请求最大大小：`1024` 字节。
- 握手超时：`5000ms`。
- TCP 控制空闲超时：`15000ms`。
- UDP 会话超时：`15000ms`。
- UDP 最大包大小：`1400` 字节。
- UDP 最大音频 payload 大小：`1200` 字节。
- 状态访问 key：`audiostatus`。

业务 `key` 规则：

- 非空。
- 最大长度为 `128` 个 UTF-8 字节。
- 按照 NVDA Remote 行为，把 key 当作不透明的密码/通道字符串，并使用原样精确匹配。
- 不要 trim，不要转小写，不要归一化，不要限制可打印符号、空格或 Unicode 字符。
- 拒绝控制字符，因为 key 会写入日志，并会暴露给按行处理的运维工具。

TCP 控制行为：

- 客户端连接后立即发送一行以 `\n` 结尾的 JSON。
- 角色为 `publisher` 或 `subscriber`。
- 成功响应包含 `status`、`role`、`key`、`session_id`、`udp_port`、`tcp_heartbeat_interval_ms`、`udp_session_timeout_ms` 和 `udp_audio_payload_max_bytes`。
- `session_id` 固定为 16 字节，序列化为 32 个十六进制字符。
- 每个成功会话必须保持自己的 TCP 控制连接存活。
- TCP 心跳 JSON 为 `{"type":"heartbeat"}`。
- TCP 控制连接断开时，必须立即让对应会话及其 UDP 端点失效。

UDP 包布局：

- Magic：`RAS1`。
- Version：`1`。
- 包类型：`0x01 register`、`0x02 register_ack`、`0x03 heartbeat`、`0x04 audio_data`。
- UDP 包内的 `session_id` 是 16 字节原始值。
- `audio_data` 元数据使用 big-endian `u64` sequence 和 big-endian `u64` 毫秒 timestamp。
- 推流端音频必须来自该 publisher 会话已注册的 UDP 端点。
- Subscriber 会话绝不能被当作音频 publisher 接受。
- 转发音频时，`sequence`、`timestamp_ms` 和 payload 保持不变，但 `session_id` 必须替换成目标 subscriber 的 session id。

## 仓库结构

- `src/config.rs`：CLI 参数、默认值、协议限制。
- `src/protocol.rs`：JSON 和 UDP 编解码辅助函数。
- `src/state.rs`：会话注册表、流状态、计数器、UDP 端点校验。
- `src/server.rs`：TCP 控制服务、UDP 服务、状态服务、分发 worker、集成测试。
- `src/net.rs`：UDP socket 绑定和缓冲区大小设置。
- `src/main.rs`：运行时入口和日志初始化。
- `src/bin/NVDARemoteAudioServer_load_test.rs`：真实 TCP/UDP 压测工具。
- `deploy/systemd/NVDARemoteAudioServer.service`：Linux systemd 服务模板。
- `.github/workflows/release.yml`：标签触发的发布流水线。

不要提交 `target/`、`dist/`、本地日志、抓包文件、临时压测输出或手动构建的二进制。发布二进制应放在 GitHub Releases。

## 必要验证

提交代码变更前运行：

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

如果修改了协议或网络逻辑，还要运行一次真实本地 TCP/UDP 压测：

```bash
cargo run --release --bin NVDARemoteAudioServer_load_test -- --publishers=20 --subscribers-per-publisher=20 --packets-per-publisher=200 --payload-bytes=1200
```

如果某个验证命令无法运行，必须明确说明跳过了哪个命令以及原因。

## 发布契约

推送标签时，GitHub Actions 会创建 release。

支持的标签风格：

- `0.1`
- `v0.1.0`

推荐发布步骤：

```bash
git status
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
git tag -a 0.1 -m "Release 0.1"
git push origin 0.1
```

CI workflow 必须：

- 运行格式检查、clippy 和测试。
- 构建 Linux amd64。
- 构建 Windows amd64。
- 将二进制与 `README.md`、`README-ZHCN.md`、`API.md`、`API-ZHCN.md` 和 `LICENSE` 一起打包。
- 根据推送的标签创建 GitHub Release。

不要把 release 二进制手动加入仓库。

## 编码规则

- 保持服务端小而可预测。
- 生产路径优先使用明确错误处理，不要使用 `unwrap` 或 `expect`。
- 测试里可以使用 `unwrap` 和 `expect`，前提是能让测试意图更清楚。
- UDP 解析时，切片前必须先做长度检查。
- 不要持锁跨越 `.await`。
- 不要在 async 热路径里加入阻塞 I/O。
- 未经过协议决策，不要给服务端增加 codec、音频、RTP、重试、重传或缓冲特性。
- 不要修改默认值或包布局，除非同时更新测试和公开文档。
- 不要削弱端点绑定：UDP 包必须匹配已注册的源地址和端口。
- 状态输出保持按行 JSON。
- 日志要对运维定位有用，但除错误路径外，不要加入高频逐包日志。

## 文档规则

以下公开说明文档必须一起更新：

- `README.md`
- `README-ZHCN.md`
- `API.md`
- `API-ZHCN.md`

以下两份智能体维护指南必须一起更新：

- `AGENTS.md`
- `AGENTS-ZHCN.md`

只要行为发生变化，就要记录：

- CLI 参数。
- 默认端口。
- 公开 TCP/UDP/状态 API 行为。
- 部署步骤。
- 状态行为。
- 压测用法。
- 如果 CI 有变化，还要记录 release 行为。

文字要实用、可读。README 应该能帮助运维人员在不理解整个代码库的情况下先把服务跑起来。

## 安全与运维说明

- 状态 key 固定为 `audiostatus`；除非防火墙规则是有意配置的，否则不要把状态端口暴露到公网。
- 当前 Linux 生产部署的 systemd unit 使用 `root` 运行，这是项目需求。如果将来改变这一点，必须同步更新 systemd 和文档。
- 需要放行 `6838` 的 TCP 和 UDP。
- 只有需要状态访问时才放行 TCP `6839`。
- Windows 上服务端是普通控制台进程，不是原生 Windows Service 二进制。除非代码已经实现 Windows Service 支持，否则不要文档化直接使用 `sc.exe create` 安装服务。

## 交付检查清单

交付前确认：

- 已理解并报告 `git status --short`。
- 没有准备把 `target/` 或 `dist/` 目录提交进仓库。
- README 和 README-ZHCN 保持同步。
- API 和 API-ZHCN 保持同步。
- AGENTS 和 AGENTS-ZHCN 保持同步。
- CI workflow 仍然在推送标签时触发。
- release 标签流程没有被破坏。
- 所有验证结果都如实报告。
