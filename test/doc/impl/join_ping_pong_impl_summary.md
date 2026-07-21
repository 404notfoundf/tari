Ping/Pong + Join/Propagate Join TTL 实现整理

本文档记录当前已经实际落地的代码改动，不是设计方案，而是实现结果摘要。


## 1. 已实现范围

当前已经实现：

1. `Ping` TTL
2. `Pong` TTL
3. `Join` TTL
4. `Propagate Join` TTL
5. `Discovery` TTL
6. outbound 发送前过期丢弃
7. outbound 转 retry 前过期丢弃

也就是说，相关消息现在不再只是“可设计为 TTL”，而是已经真正进入了过期清理链路。


## 2. 核心实现思路

这次没有单独为每种消息各写一套过期逻辑，而是统一走：

1. 发送源头写入 `expires`
2. DHT outbound 透传 `expires`
3. 序列化成 `OutboundMessage` 时缓存 `expires_at`
4. `messaging/outbound` 发送前判断是否过期
5. `messaging/outbound` 转 retry 前再次判断是否过期

这样所有带 `expires` 的消息，都能复用同一套清理机制。


## 3. 实际修改文件

### 3.1 `comms/core/src/message/outbound.rs`

已实现：

- `OutboundMessage` 增加：

```rust
pub expires_at: Option<u64>
```

- 增加：

```rust
pub fn is_expired(&self) -> bool
```

作用：

- 让 comms messaging 层不需要解析业务消息体，也能直接判断是否过期


### 3.2 `comms/dht/src/outbound/message_params.rs`

已实现：

- `FinalSendMessageParams` 增加：

```rust
pub expires: Option<EpochTime>
```

- `SendMessageParams` 增加：

```rust
with_expires(...)
```

作用：

- 让业务发送点可以显式设置消息 TTL


### 3.3 `comms/dht/src/outbound/broadcast.rs`

已实现：

- `handle_send_message(...)` 解构 `FinalSendMessageParams` 时拿到 `expires`
- 如果调用方未传 `expires`，保持原有默认：
  - `Utc::now() + self.message_validity_window`
- `generate_send_messages(...)` 改为直接接收 `Option<EpochTime>`

作用：

- 保持原有默认过期行为
- 同时允许特定消息覆盖默认 TTL


### 3.4 `comms/dht/src/outbound/serialize.rs`

已实现：

- 构造 `OutboundMessage` 时，把 `DhtOutboundMessage.expires` 写入：

```rust
expires_at: expires
```

作用：

- 把 DHT 层过期时间带到 comms messaging 层


### 3.5 `comms/core/src/protocol/messaging/outbound.rs`

已实现两处关键清理：

#### 发送前清理

- 在真正把消息 body 送到 sink 之前：
  - 如果 `out_msg.is_expired()`
  - 直接 `reply_fail(SendFailReason::Dropped)`
  - 不再发送

#### retry 前清理

- disconnect 后 draining 剩余消息时：
  - 如果 `msg.is_expired()`
  - 直接失败并丢弃
  - 不再进入 retry queue

作用：

- 防止旧消息真正发到网络
- 防止旧消息在 retry 里继续堆积


## 4. Ping/Pong 实际实现

### 4.1 `base_layer/p2p/src/services/liveness/service.rs`

已实现：

- `send_ping(...)`
  - 不再直接调用 `send_direct_node_id(...)`
  - 改为手动构造 `SendMessageParams`
  - 写入：

```rust
with_expires(EpochTime::from_secs_since_epoch(EpochTime::now().as_u64() + ttl.as_secs()))
```

- `Ping` 使用 TTL：
  - `auto_ping_interval`
  - 如果没有配置，则回退到 `MAX_INFLIGHT_TTL`

### 4.2 `send_pong(...)`

已实现：

- 不再直接调用 `send_direct_unencrypted(...)`
- 改为手动构造 `SendMessageParams`
- TTL 固定为：

```rust
Duration::from_secs(5)
```

效果：

- `Ping/Pong` 现在会携带明确 TTL
- 旧 `Ping/Pong` 不会无限保留


## 5. Join / Propagate Join 实际实现

### 5.1 `comms/dht/src/actor.rs`

已实现：

- 增加：

```rust
const JOIN_MESSAGE_TTL_SECS: u64 = 30;
```

- 在广播 `Join` 的发送参数中增加：

```rust
.with_expires(EpochTime::from_secs_since_epoch(EpochTime::now().as_u64() + JOIN_MESSAGE_TTL_SECS))
```

效果：

- `Join` 广播现在有 `30s` TTL


### 5.2 `comms/dht/src/inbound/dht_handler/task.rs`

已实现：

- 增加：

```rust
const PROPAGATE_JOIN_MESSAGE_TTL_SECS: u64 = 10;
```

- 在 `send_raw_no_wait(...)` 传播 Join 的发送参数中增加：

```rust
.with_expires(...)
```

效果：

- `Propagate Join` 现在有 `10s` TTL


### 5.3 `comms/dht/src/discovery/service.rs`

已实现：

- 增加：

```rust
const DISCOVERY_MESSAGE_TTL_SECS: u64 = 30;
```

- 在 `send_discover(...)` 的 `Discovery` 发送参数中增加：

```rust
.with_expires(EpochTime::from_secs_since_epoch(
    EpochTime::now().as_u64() + DISCOVERY_MESSAGE_TTL_SECS,
))
```

效果：

- `Discovery` 现在有 `30s` TTL


## 6. 当前已生效的 TTL 值

当前代码中已经落地的 TTL：

- `Ping`
  - `auto_ping_interval`
  - 未配置则 `MAX_INFLIGHT_TTL`

- `Pong`
  - `5s`

- `Join`
  - `30s`

- `Propagate Join`
  - `10s`

- `Discovery`
  - `30s`


## 7. 当前行为变化

实现后，这几类消息会出现下面的行为变化：

### 7.1 正常快速发送

- 行为与之前基本一致

### 7.2 排队过久

- 如果超过 TTL：
  - 不再真正发送
  - 直接失败并释放

### 7.3 断连后进入 retry

- 如果消息还没过期：
  - 仍可进入 retry

- 如果已经过期：
  - 不再进入 retry
  - 直接丢弃


## 8. 这次没有做的事

当前还没有实现：

- `DiscoveryResponse` 的特殊失败策略
- `Expired` 专用 `SendFailReason`
- 过期丢弃计数 metrics

当前统一复用的是：

- `SendFailReason::Dropped`


## 9. 验证情况

已尝试运行：

```powershell
cargo check -p tari_p2p -p tari_comms -p tari_comms_dht --features metrics
```

当前环境仍然因为缺少 `link.exe` 失败，属于本机 MSVC 工具链问题，不是本次改动定位到的 Rust 语法错误输出。


## 10. 一句话结论

现在 `Ping/Pong/Join/Propagate Join/Discovery` 都已经接入 TTL：

- 发送时带过期时间
- 发送前会清理过期消息
- retry 前也会清理过期消息

这能直接减少这些强时效 / 传播型消息长期滞留导致的 `OutboundMessage` 累积。
