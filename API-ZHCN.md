# NVDARemoteAudioServer API

本文说明 `NVDARemoteAudioServer` 的网络协议，面向需要接入服务端的客户端开发人员。

服务端不是 HTTP 服务。它提供：

- TCP 控制 API，用于认证和心跳。
- UDP 二进制 API，用于端点注册、UDP 心跳和音频包转发。
- TCP 状态 API，用于获取运行状态快照。

服务端不采集、编码、解码、播放、混音、重采样、重传、排序或修复音频数据。所有音频处理都由客户端负责。

## 默认端点

| 用途 | 传输协议 | 默认监听地址 |
| --- | --- | --- |
| 控制握手和控制心跳 | TCP | `0.0.0.0:6838` |
| UDP 注册、UDP 心跳、音频数据 | UDP | `0.0.0.0:6838` |
| 状态快照 | TCP | `0.0.0.0:6839` |

启动参数：

```bash
NVDARemoteAudioServer --port=6838 --sport=6839 --log=/home/app/NVDARemoteAudioServer/logs/NVDARemoteAudioServer.log
```

| 参数 | 含义 |
| --- | --- |
| `--port=6838` | 同时设置 TCP 控制端口和 UDP 数据端口。 |
| `--sport=6839` | 设置 TCP 状态端口。 |
| `--log=/path/to/file.log` | 将日志写入文件。不传时日志输出到 stdout。 |

## 协议常量

| 名称 | 值 |
| --- | --- |
| TCP 握手最大请求大小 | `4096` 字节 |
| TCP 控制消息最大请求大小 | `1024` 字节 |
| TCP 状态请求最大大小 | `1024` 字节 |
| 内置工具使用的 TCP 状态响应最大大小 | `16777216` 字节 |
| TCP 握手超时 | `5000ms` |
| TCP 控制空闲超时 | `15000ms` |
| 服务端返回给客户端的 TCP 心跳间隔 | `5000ms` |
| UDP 会话超时 | `15000ms` |
| UDP 最大包大小 | `1400` 字节 |
| UDP 最大音频 payload 大小 | `1200` 字节 |
| UDP magic | ASCII `RAS1` |
| UDP version | `1` |
| 状态访问 key | `audiostatus` |

## Key 规则

业务 `key` 是把一个推流端和多个拉流端绑定在一起的密码/通道字符串。

规则：

- 不能为空。
- 最大长度为 `128` 个 UTF-8 字节。
- 按原样精确匹配。
- 不会 trim，不会转小写，不会 Unicode 归一化，也不限制为 ASCII。
- 可以包含可打印空格、符号和 Unicode 字符。
- 不能包含控制字符，例如换行、制表符、escape 或其他 Unicode 控制码位。

客户端接入注意：把 key 当作普通 JSON 字符串编码。引号、反斜杠、Unicode 等转义应交给 JSON 编码器处理。

## TCP 控制 API

每个推流端和每个拉流端都必须在整个会话生命周期内保持自己的 TCP 控制连接。

如果 TCP 控制连接断开，服务端会立即让对应会话和 UDP 端点失效。

### 握手请求

连接控制端口后，立即发送一个 UTF-8 JSON 对象，并以 `\n` 结尾。

推流端：

```json
{"role":"publisher","key":"room-123"}
```

拉流端：

```json
{"role":"subscriber","key":"room-123"}
```

字段：

| 字段 | 类型 | 必填 | 含义 |
| --- | --- | --- | --- |
| `role` | string | 是 | `publisher` 或 `subscriber`。 |
| `key` | string | 是 | 业务 key/密码/通道字符串。 |

请求帧格式：

- JSON 请求按行读取，必须以 `\n` 结束。
- `\r\n` 可接受，因为读取时会忽略 `\r`。
- 空行会被拒绝。
- 超过 `4096` 字节的请求会被拒绝。
- 客户端连接后如果没有在 `5000ms` 内完成握手，握手失败。

### 握手成功响应

服务端返回一行 JSON：

```json
{"status":"ok","message":"control session established","role":"publisher","key":"room-123","session_id":"00112233445566778899aabbccddeeff","udp_port":6838,"tcp_heartbeat_interval_ms":5000,"udp_session_timeout_ms":15000,"udp_audio_payload_max_bytes":1200}
```

字段：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `status` | string | 成功时为 `ok`。 |
| `message` | string | 人类可读状态信息。 |
| `role` | string | 已接受的角色。 |
| `key` | string | 已接受的 key。 |
| `session_id` | string | 32 个十六进制字符，表示 16 字节原始值。 |
| `udp_port` | number | UDP register、UDP heartbeat 和 audio 使用的端口。 |
| `tcp_heartbeat_interval_ms` | number | 推荐 TCP 心跳间隔。 |
| `udp_session_timeout_ms` | number | UDP 端点空闲超时。 |
| `udp_audio_payload_max_bytes` | number | 单个 UDP 音频包的最大 payload 字节数。 |

客户端接入注意：把 32 字符 `session_id` 十六进制字符串转换成 16 字节原始值后再放入 UDP 包。

### 握手失败行为

如果同一个 `key` 已经有活跃 publisher，第二个 publisher 会被拒绝，并返回一行 JSON：

```json
{"status":"error","message":"publisher already connected for this key","key":"room-123"}
```

其他错误握手、非法 key、非法 JSON、超长请求或超时，并不是稳定的 JSON 错误 API。客户端应把它们视为连接失败，修正请求后再重连。

### TCP 控制心跳

握手成功后，在同一个 TCP 连接上周期性发送这一行 JSON：

```json
{"type":"heartbeat"}
```

行为：

- 服务端不会返回心跳响应。
- 正常心跳间隔使用握手响应里的 `tcp_heartbeat_interval_ms`。
- 如果 `15000ms` 内没有收到有效控制消息，服务端会关闭会话。
- 一个 TCP 连接只绑定一个角色和一个 key。

## UDP 二进制 API

所有 UDP 包都使用同一个基础头。

### 基础头

| 偏移 | 大小 | 字段 | 值 |
| --- | ---: | --- | --- |
| `0` | `4` | magic | ASCII `RAS1` |
| `4` | `1` | version | `0x01` |
| `5` | `1` | packet_type | 见包类型表。 |
| `6` | `16` | session_id | TCP 握手 `session_id` 对应的 16 字节原始值。 |

基础头长度：`22` 字节。

包类型：

| 类型 | 名称 | 方向 |
| --- | --- | --- |
| `0x01` | `register` | 客户端到服务端 |
| `0x02` | `register_ack` | 服务端到客户端 |
| `0x03` | `heartbeat` | 客户端到服务端 |
| `0x04` | `audio_data` | 推流端到服务端，服务端到拉流端 |

magic 错误、version 不支持、未知类型、长度错误或 payload 超过限制的包会被拒绝，并计入 invalid UDP packet 统计。

### UDP Register

Register 包布局：

| 偏移 | 大小 | 字段 |
| --- | ---: | --- |
| `0` | `4` | magic `RAS1` |
| `4` | `1` | version `0x01` |
| `5` | `1` | packet_type `0x01` |
| `6` | `16` | session_id |

总大小：`22` 字节。

TCP 握手成功后，每个推流端和拉流端都必须从自己的 UDP socket 向服务端 UDP 端口发送 `register`。

服务端行为：

- 如果 `session_id` 已知，并且对应 TCP 控制会话仍存活，服务端会把该会话绑定到 UDP 源 IP 和源端口。
- 服务端向同一个 UDP 端点回复 `register_ack`。
- 如果 `session_id` 未知，或 TCP 会话已经结束，注册会被拒绝且不会发送 `register_ack`。
- 同一个存活会话后续再次发送有效 `register` 会更新 UDP 端点。客户端的 UDP 源端口变化后，用这种方式恢复。

### UDP Register Ack

Register ack 包布局：

| 偏移 | 大小 | 字段 |
| --- | ---: | --- |
| `0` | `4` | magic `RAS1` |
| `4` | `1` | version `0x01` |
| `5` | `1` | packet_type `0x02` |
| `6` | `16` | session_id |

总大小：`22` 字节。

客户端行为：

- Publisher 收到 `register_ack` 前不要发送音频。
- Subscriber 收到 `register_ack` 前不要认为自己已准备好接收。
- 如果 UDP 源端口变化，重新发送 `register` 并等待新的 `register_ack`。

### UDP Heartbeat

Heartbeat 包布局：

| 偏移 | 大小 | 字段 |
| --- | ---: | --- |
| `0` | `4` | magic `RAS1` |
| `4` | `1` | version `0x01` |
| `5` | `1` | packet_type `0x03` |
| `6` | `16` | session_id |

总大小：`22` 字节。

行为：

- Publisher 和 subscriber 都应周期性发送 UDP heartbeat。
- 服务端不会返回 UDP heartbeat 响应。
- 心跳必须来自当前已注册的 UDP 源端点。
- 已注册 UDP 端点如果 `15000ms` 无活动，会被视为过期。
- 端点过期或源端口变化后，重新发送 `register` 并等待 `register_ack`。

### UDP Audio Data

音频包布局：

| 偏移 | 大小 | 字段 | 编码 |
| --- | ---: | --- | --- |
| `0` | `4` | magic | ASCII `RAS1` |
| `4` | `1` | version | `0x01` |
| `5` | `1` | packet_type | `0x04` |
| `6` | `16` | session_id | 16 字节原始值 |
| `22` | `8` | sequence | big-endian `u64` |
| `30` | `8` | timestamp_ms | big-endian `u64` |
| `38` | `N` | payload | 客户端自定义不透明音频字节 |

最小大小：`38` 字节。

最大 payload 大小：`1200` 字节。

服务端接收缓冲使用的最大 UDP 包大小：`1400` 字节。

Publisher 到服务端规则：

- 只有 `publisher` 会话可以发送 `audio_data`。
- TCP 控制会话必须仍然存活。
- Publisher UDP 端点必须已经成功注册。
- 包必须来自注册该会话时相同的 UDP 源 IP 和端口。
- `session_id` 必须是 publisher 自己的 session id。
- 除大小和布局检查外，服务端不理解 `sequence`、`timestamp_ms` 和 `payload`。

服务端到 subscriber 的转发规则：

- 服务端只转发给同一个 `key` 下已注册且仍活跃的 subscriber UDP 端点。
- 转发包保持 `sequence` 不变。
- 转发包保持 `timestamp_ms` 不变。
- 转发包保持 `payload` 不变。
- 转发包会把 `session_id` 替换为目标 subscriber 自己的 session id。

服务端不会确认音频包，也不会重传丢失的音频包。

## 必要客户端流程

### Publisher 流程

1. 连接 TCP 控制端口。
2. 发送握手 JSON 行：`{"role":"publisher","key":"..."}`。
3. 读取一行 JSON 响应。
4. 确认 `status == "ok"`。
5. 保存 `session_id`、`udp_port`、`tcp_heartbeat_interval_ms`、`udp_session_timeout_ms` 和 `udp_audio_payload_max_bytes`。
6. 打开 UDP socket。
7. 使用 16 字节 session id 发送 UDP `register`。
8. 等待 `register_ack`。
9. 在 TCP 控制连接上持续发送 TCP heartbeat。
10. 从已注册的 UDP socket 持续发送 UDP heartbeat。
11. 从同一个 UDP socket 发送 UDP `audio_data`。

### Subscriber 流程

1. 连接 TCP 控制端口。
2. 发送握手 JSON 行：`{"role":"subscriber","key":"..."}`。
3. 读取一行 JSON 响应。
4. 确认 `status == "ok"`。
5. 保存 `session_id`、`udp_port`、`tcp_heartbeat_interval_ms` 和 `udp_session_timeout_ms`。
6. 打开 UDP socket。
7. 使用 16 字节 session id 发送 UDP `register`。
8. 等待 `register_ack`。
9. 在 TCP 控制连接上持续发送 TCP heartbeat。
10. 从已注册的 UDP socket 持续发送 UDP heartbeat。
11. 读取 UDP `audio_data`。
12. 验证收到的转发 `audio_data` 使用的是 subscriber 自己的 session id。

## 状态 API

状态 API 使用独立 TCP 端口。

发送一行 JSON：

```json
{"key":"audiostatus"}
```

成功响应是一行 JSON，内容是 `RegistrySnapshot` 对象：

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

状态 key 错误响应：

```json
{"status":"error","message":"invalid status key"}
```

状态请求格式错误响应：

```json
{"status":"error","message":"invalid status request"}
```

状态客户端要求：

- 发送一行以 `\n` 结尾的 JSON。
- 请求大小不能超过 `1024` 字节。
- 读取完整一行 JSON，直到换行符。
- 不要假设响应能放进 4 KB 缓冲区。大规模部署可能产生明显更大的快照。

## 错误与恢复建议

推荐客户端行为：

- 如果 TCP 控制连接断开，把整个会话视为失效，并从 TCP 握手重新开始。
- 如果没有收到 UDP `register_ack`，在保持 TCP 存活的同时带退避重试 `register`。
- 如果 UDP 源端口变化，重新发送 `register` 并等待 `register_ack`。
- 如果 subscriber 音频停止但 TCP 仍存活，继续发送 TCP 和 UDP 心跳；如果怀疑 NAT 或端口变化，重新注册 UDP。
- 如果服务端拒绝或忽略 UDP 音频，检查 role、session id 和 UDP 源端点绑定。

## 最小 UDP 编码参考

UDP 音频包伪代码：

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

UDP register 包伪代码：

```text
packet = bytes()
packet += ASCII("RAS1")
packet += u8(1)
packet += u8(0x01)
packet += session_id_16_bytes
```
