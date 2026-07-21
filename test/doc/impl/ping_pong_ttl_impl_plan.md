Ping/Pong TTL 最小实现清单

本文档给出一版最小改动实现方案，目标是：

- `Ping/Pong` 可以短暂进入 retry
- 但过期后必须直接丢弃
- 避免这类强时效消息长期占用 `OutboundMessage`

这版方案尽量复用现有 DHT `expires` 机制，不先大改 `OutboundMessage` 结构。


## 1. 设计目标

要解决的问题不是“所有消息都不能 retry”，而是：

- `Ping/Pong` 可以容忍短暂重试
- 但不能无限积压
- 过期后继续发送已经没有意义

因此第一版实现目标是：

1. `Ping/Pong` 发送时带上过期时间
2. 发送前检查是否过期
3. 断连后转 retry 前再次检查是否过期
4. 过期则直接丢弃并失败返回


## 2. 为什么选这个方案

当前代码里已经有现成的 DHT 过期字段：

- `comms/dht/src/outbound/message.rs:177`
  - `pub expires: Option<u64>`

现有序列化也已经会把过期时间写入 DHT header：

- `comms/dht/src/outbound/serialize.rs:81`
- `comms/dht/src/outbound/serialize.rs:98`

inbound 侧也已经有“过期就丢”的逻辑风格：

- `comms/dht/src/dht.rs:380`

所以最小方案不是新增一套 TTL 体系，而是：

- 让 `Ping/Pong` 走现有 `expires`
- 在 outbound 发送链路补齐过期检查


## 3. 需要修改的文件

### 3.1 `comms/dht/src/outbound/message_params.rs`

目的：

- 给发送参数增加 `expires`

建议修改：

- 在 `FinalSendMessageParams` 中新增：

```rust
pub expires: Option<EpochTime>,
```

- 在 `Default` 中初始化为：

```rust
expires: None,
```

- 增加 builder：

```rust
pub fn with_expires(&mut self, expires: EpochTime) -> &mut Self
```


### 3.2 DHT outbound 组装消息的位置

目的：

- 把 `params.expires` 透传到 `DhtOutboundMessage.expires`

需要检查并修改 DHT outbound 从 `FinalSendMessageParams` 构造 `DhtOutboundMessage` 的位置。

最终要求：

- `DhtOutboundMessage.expires` 能拿到 `params.expires.map(EpochTime::as_u64)`

相关结构：

- `comms/dht/src/outbound/message.rs:164`
- `comms/dht/src/outbound/serialize.rs:69`


### 3.3 `base_layer/p2p/src/services/liveness/service.rs`

目的：

- `send_ping` / `send_pong` 发消息时写入 TTL

当前发送点：

- `base_layer/p2p/src/services/liveness/service.rs:255`
- `base_layer/p2p/src/services/liveness/service.rs:276`

当前问题：

- 现在 `send_direct_node_id(...)` / `send_direct_unencrypted(...)` 没有传 `expires`

建议改法：

- 把这两处改成手动构造 `SendMessageParams`
- 再调用：
  - `send_message(...)`
  - 或 `send_message_no_header(...)`

示意：

```rust
let expires = EpochTime::now() + ttl;
let params = SendMessageParams::new()
    .direct_node_id(node_id.clone())
    .with_debug_info("Send ping".to_string())
    .with_expires(expires)
    .finish();

self.outbound_messaging
    .send_message(params, OutboundDomainMessage::new(&TariMessageType::PingPong, msg))
    .await?;
```

`Pong` 同理。


### 3.4 `comms/core/src/protocol/messaging/outbound.rs`

目的：

- 在真正发送前丢弃过期消息
- 在转 retry 前再次丢弃过期消息

这是关键修改点。


## 4. outbound 侧具体实现

### 4.1 增加过期判断 helper

文件：

- `comms/core/src/protocol/messaging/outbound.rs`

建议增加一个私有 helper，例如：

```rust
fn is_expired_message(msg: &OutboundMessage) -> bool
```

实现思路：

1. 尝试把 `msg.body` decode 成 `DhtEnvelope`
2. 读取 `header.expires`
3. 如果：
   - `expires` 存在
   - 且 `< EpochTime::now()`
   - 返回 `true`
4. 其余情况返回 `false`

参考现有过期判断：

- `comms/dht/src/dht.rs:380`


### 4.2 在真正发送前检查过期

位置：

- `comms/core/src/protocol/messaging/outbound.rs:272`

当前逻辑：

- 从 `messages_rx` 拿到 `out_msg`
- 立即 `reply_success()`
- 然后把 `body` 送进 sink

建议改成：

1. 先判断 `is_expired_message(&out_msg)`
2. 如果已过期：
   - `reply_fail(...)`
   - 不发送
   - 直接跳过
3. 如果未过期：
   - 保持原有发送逻辑

注意：

- 这一步非常重要
- 否则 retry 回来的旧 `Ping/Pong` 仍然会被真正写到网络里


### 4.3 在 disconnect 后转 retry 前再次检查

位置：

- `comms/core/src/protocol/messaging/outbound.rs:315`

当前逻辑：

- 断连后剩余消息一股脑转进 retry queue

建议改成：

1. 从 `messages_rx` 取出 `msg`
2. 先判断 `is_expired_message(&msg)`
3. 如果已过期：
   - `reply_fail(...)`
   - 不进入 retry
4. 如果未过期：
   - 进入 retry queue

这样即使消息已经排队很久，也不会继续占用 retry 空间。


## 5. 建议 TTL 数值

第一版先保守一些：

- `Ping`
  - `ttl = auto_ping_interval`
  - 如果没有配置 `auto_ping_interval`，用 `30s`

- `Pong`
  - `ttl = 5s`

理由：

- `Ping` 是周期探测，下一轮会自然替代
- `Pong` 是即时响应，比 `Ping` 更应该短


## 6. 是否需要改 `OutboundMessage`

第一版建议：

- **不改**

原因：

- 当前 `body` 里已经包含 DHT envelope
- 可以直接从 `body` 解出 `expires`
- 功能先做通，改动最小

后续如果验证 decode 开销值得优化，再考虑：

- 给 `OutboundMessage` 增加缓存字段
  - 比如 `expires_at: Option<u64>`

但那属于第二阶段优化，不是第一版必须项。


## 7. 失败原因建议

第一版可以直接复用现有：

- `SendFailReason::Dropped`

如果后续想把 metrics / 日志做得更清楚，可以再新增：

- `SendFailReason::Expired`

但这不是第一版必须项。


## 8. 测试建议

建议至少补两类测试：

### 8.1 过期消息不会真正发送

思路：

- 构造带 `expires = now - 1s` 的消息
- 放入 outbound path
- 断言：
  - 不会真正写到 sink
  - 会失败返回


### 8.2 断连后过期消息不会进入 retry

思路：

- 构造一条即将过期或已过期的 `Ping/Pong`
- 制造 disconnect
- 断言：
  - 不进入 retry queue
  - `retry_queue_messages` 不增加


## 9. 第一版最小实施顺序

建议按这个顺序落地：

1. `message_params.rs`
   - 增加 `expires`
   - 增加 `with_expires`

2. `liveness/service.rs`
   - `send_ping` / `send_pong` 写入 TTL

3. `messaging/outbound.rs`
   - 增加 `is_expired_message`
   - 发送前检查
   - 转 retry 前检查

4. 补测试


## 10. 一句话结论

最小实现不是直接禁止 `Ping/Pong` retry，而是：

- 允许短暂 retry
- 用现有 DHT `expires` 给 `Ping/Pong` 加短 TTL
- 在 outbound 发送前和转 retry 前做两次过期检查
- 过期就直接丢弃

这能在不大改架构的前提下，显著降低 `Ping/Pong` 导致的 `OutboundMessage` 长时间滞留。
