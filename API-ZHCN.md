# NVDARemoteAudioServer API

[English](API.md) · [部署与使用](README-ZHCN.md)

接口直接使用 TCP/UDP，不使用 HTTP。

## 端口与限制

| 接口 | 默认监听地址 | 用途 |
| --- | --- | --- |
| TCP 控制 | `0.0.0.0:6838` | 握手和会话心跳 |
| UDP 数据 | `0.0.0.0:6838` | 端点注册、心跳和音频 |
| TCP 状态 | `0.0.0.0:6839` | 状态查询 |

| 限制 | 值 |
| --- | --- |
| TCP 握手请求 | 4096 字节 |
| TCP 控制消息 | 1024 字节 |
| TCP 状态请求 | 1024 字节 |
| 握手和状态请求超时 | 5000 ms |
| TCP 控制空闲超时 | 15000 ms |
| 建议 TCP 心跳间隔 | 5000 ms |
| UDP 端点空闲超时 | 15000 ms |
| UDP 单包接收缓冲区 | 1400 字节 |
| 音频载荷上限 | 1200 字节 |

TCP 请求和响应均为 UTF-8 JSON 对象，每行一个，以 `\n` 结束。请求也接受 `\r\n`。大小限制不计结束换行符和读取时忽略的回车符。空行无效。

## 连接密钥与音频流

`key` 是通道的共享密码，按 NVDA Remote 的规则原样匹配：

- 非空，长度不超过 128 个 UTF-8 字节。
- 允许空格、符号和 Unicode 字符；不去除空白、不转换大小写、不做 Unicode 归一化。
- 不允许控制字符，包括换行符、制表符和转义控制字符。

`stream` 为必填字段，可选值如下：

| 值 | 方向 |
| --- | --- |
| `system_audio` | 被控端系统音频 → 控制端 |
| `voice_controlled_to_controller` | 被控端麦克风 → 控制端 |
| `voice_controller_to_controlled` | 控制端麦克风 → 被控端 |

每个 `(key, stream)` 最多允许一个推流端，可有多个拉流端。拉流端可先连接。推流端的 TCP 会话结束前，该流不接受第二个推流端，与 UDP 是否注册或过期无关。

## TCP 控制 API

每个推流端和拉流端须保持独立的 TCP 连接，绑定一个角色、一个密钥和一条流。服务端检测到断开或控制超时后，会话及 UDP 端点立即失效；重连须重新握手。

### 握手

连接后立即发送握手请求，并在 5000 ms 内完成。`role`、`key`、`stream` 均为必填字符串；`role` 为 `publisher`（推流端）或 `subscriber`（拉流端）。

```json
{"role":"publisher","key":"room-123","stream":"system_audio"}
```

成功响应：

```json
{"status":"ok","message":"control session established","role":"publisher","key":"room-123","stream":"system_audio","session_id":"00112233445566778899aabbccddeeff","udp_port":6838,"tcp_heartbeat_interval_ms":5000,"udp_session_timeout_ms":15000,"udp_audio_payload_max_bytes":1200}
```

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `status` | string | 成功时为 `ok` |
| `message` | string | 状态说明 |
| `role`, `key`, `stream` | string | 已接受的握手字段 |
| `session_id` | string | 16 字节会话 ID，编码为 32 个小写十六进制字符 |
| `udp_port` | integer | 所有 UDP 包使用的服务端端口 |
| `tcp_heartbeat_interval_ms` | integer | 建议 TCP 心跳间隔 |
| `udp_session_timeout_ms` | integer | UDP 端点空闲超时 |
| `udp_audio_payload_max_bytes` | integer | 单包音频载荷上限 |

同一 `(key, stream)` 已有推流端时，服务端返回以下错误并关闭连接：

```json
{"status":"error","message":"publisher already connected for this key and stream","key":"room-123","stream":"system_audio"}
```

其他无效或超时的握手直接关闭连接，不返回 JSON 错误响应。

### 心跳

按握手响应给出的间隔，在同一连接上发送：

```json
{"type":"heartbeat"}
```

服务端不回复心跳。15000 ms 内未收到完整有效的心跳，会话将关闭；无效或过大的控制消息也会关闭会话。UDP 流量不能维持 TCP 会话。

## UDP 二进制 API

### 公共包头

下表偏移和大小均以字节为单位。所有包都以这个 22 字节包头开始：

| 偏移 | 大小 | 字段 | 值 |
| --- | ---: | --- | --- |
| 0 | 4 | `magic` | ASCII `RAS1` |
| 4 | 1 | `version` | `0x01` |
| 5 | 1 | `packet_type` | 见下表 |
| 6 | 16 | `session_id` | TCP 握手所得会话 ID 的原始字节 |

| 类型 | 名称 | 方向 | 总大小 |
| --- | --- | --- | --- |
| `0x01` | `register` | 客户端 → 服务端 | 22 字节 |
| `0x02` | `register_ack` | 服务端 → 客户端 | 22 字节 |
| `0x03` | `heartbeat` | 客户端 → 服务端 | 22 字节 |
| `0x04` | `audio_data` | 推流端 → 服务端 → 拉流端 | 38–1238 字节 |

前三种包仅含公共包头。无效包直接丢弃，错误计数见状态 API。

### 端点注册

TCP 握手后，从该会话收发 UDP 数据的套接字向 `udp_port` 发送 `register`。服务端绑定源 IP 地址和端口，并回复带有同一会话 ID 的 `register_ack`。未知或已结束的会话不会收到确认。

收到匹配的 `register_ack` 后再使用该端点；未收到时，保持 TCP 连接并按退避间隔重试。新注册会替换端点，端点过期或源地址、端口变化后须重新注册。心跳和音频包不能改变绑定。

### UDP 心跳与过期

两种角色都应从已注册的套接字定期发送 UDP `heartbeat`，例如每 5000 ms 一次。服务端不回复。

有效的注册、UDP 心跳或推流端音频包会刷新 UDP 活动时间。转发音频和 TCP 心跳不会刷新，因此拉流端必须主动发送 UDP 心跳。

超过 15000 ms 没有有效 UDP 活动，端点即过期。重新注册前，服务端拒绝该端点的心跳和音频，也不再向其转发音频。UDP 过期不会结束 TCP 会话。

### 音频包

`audio_data` 在公共包头之后增加以下字段：

| 偏移 | 大小 | 字段 | 编码 |
| --- | ---: | --- | --- |
| 22 | 8 | `sequence` | 大端序无符号 64 位整数 |
| 30 | 8 | `timestamp_ms` | 大端序无符号 64 位整数，单位为毫秒 |
| 38 | 0–1200 | `payload` | 客户端定义的音频字节 |

仅存活的 `publisher` 会话可发送音频，源 IP 地址和端口须与已注册且未过期的端点一致。

服务端仅向同一 `(key, stream)` 下已注册且未过期的拉流端转发音频，保留 `sequence`、`timestamp_ms` 和 `payload`，将 `session_id` 替换为接收方的 ID；拉流端应核对该 ID。服务端不检查音频格式或序列号、时间戳是否递增。

音频包没有确认和重传机制。转发队列满时可能丢包，服务端不保证送达或顺序。

## 状态 API

向状态端口发送以下请求，访问密钥固定：

```json
{"key":"audiostatus"}
```

服务端返回一行 JSON 后关闭连接。响应含连接密钥，应仅向可信主机开放。

成功响应（展开显示）：

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

### 全局字段

| 字段 | 含义 |
| --- | --- |
| `generated_at_unix_ms` | 快照生成时间，Unix 毫秒时间戳 |
| `stream_count`, `streams` | 当前 `(key, stream)` 记录；每个密钥最多对应三条 |
| `active_publisher_count`, `active_subscriber_count` | TCP 控制连接存活的会话数 |
| `active_udp_publisher_count`, `active_udp_subscriber_count` | UDP 端点已注册且未过期的会话数 |
| `invalid_udp_packets_total` | 格式无效的 UDP 包和客户端误发的 `register_ack` 包数 |
| `unknown_udp_session_packets_total` | 会话 ID 未知的注册、心跳和音频包数 |

两个全局 UDP 错误计数在服务端进程存活期间累计。角色错误、端点未注册、端点不匹配或过期导致的拒绝会写入日志，但不增加这两个计数。

### 每条流的字段

| 字段 | 含义 |
| --- | --- |
| `key`, `stream` | 路由组合 |
| `publisher_control_connected`, `subscriber_count` | 当前推流端 TCP 连接状态和拉流端 TCP 连接数 |
| `publisher_udp_registered`, `subscriber_udp_registered_count` | 当前未过期的 UDP 注册状态或数量 |
| `publisher_connections_total`, `subscriber_connections_total` | 控制会话注册次数 |
| `publisher_tcp_heartbeats_total`, `subscriber_tcp_heartbeats_total` | 已接受的 TCP 心跳数 |
| `publisher_udp_registers_total`, `subscriber_udp_registers_total` | 已接受的 UDP 注册次数，含重新注册 |
| `publisher_udp_heartbeats_total`, `subscriber_udp_heartbeats_total` | 已接受的 UDP 心跳数 |
| `udp_audio_packets_in_total`, `udp_audio_bytes_in_total` | 已接受的推流端音频包数和载荷字节数 |
| `udp_audio_packets_out_total`, `udp_audio_bytes_out_total` | 成功发送的音频包数和载荷字节数，按接收方分别计数 |
| `udp_send_errors_total` | 注册确认发送失败和音频转发错误；队列满时按未转发的接收方数量计数 |
| `last_activity_unix_ms` | 最近一次记录的流活动时间，Unix 毫秒时间戳 |
| `last_publisher_disconnect_reason`, `last_subscriber_disconnect_reason` | 各角色最近一次断开原因，无记录时省略；常见值为 `peer_disconnected`、`timeout`、`protocol_error` |

字节计数不含协议头。UDP 发送成功不代表接收方已收到。最后一个 TCP 会话断开后，该流及其计数被删除。

### 错误与响应大小

访问密钥错误：

```json
{"status":"error","message":"invalid status key"}
```

请求格式错误、过大或超时：

```json
{"status":"error","message":"invalid status request"}
```

响应须读取到换行符。服务端没有固定的响应大小上限；压测工具的读取上限为 16 MiB（16777216 字节）。
