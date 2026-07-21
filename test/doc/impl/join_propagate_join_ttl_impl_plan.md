Join / Propagate Join TTL 最小实现清单

本文档是 `Ping/Pong TTL` 方案的延伸，目标是：

- 让 `Join` 和 `Propagate Join` 也支持按过期时间清理
- 避免这类 gossip / 传播型消息长期滞留在 queue / retry 中
- 尽量复用同一套 `expires + outbound 过期检查` 机制


## 1. 结论

`Join` 和 `Propagate Join` 都适合做 TTL 清理。

其中：

- `Propagate Join` 更适合优先做，语义更偏 best-effort
- `Join` 也适合做，但 TTL 应比 `Propagate Join` 稍宽松

推荐第一版：

- `Propagate Join`: `10s ~ 15s`
- `Join`: `30s`


## 2. 为什么这两类消息适合 TTL

### 2.1 `Join`

`Join` 的作用是：

- 通知网络“我来了”
- 帮助其他节点感知这个节点

它有业务价值，但不是严格 request/response。

特点：

- 广播 / gossip 语义明显
- 后续通常还会再次发送
- 旧 join 的价值会快速下降

如果一条 `Join` 在队列里卡了很久才发出去：

- 对端很可能已经通过别的路径知道该节点
- 继续占用队列和 retry 空间，收益很低


### 2.2 `Propagate Join`

`Propagate Join` 本质上是 Join 的传播副本。

特点：

- 更偏传播型消息
- `no_wait` 语义更强
- 本来就属于 best-effort 候选
- 旧消息的保留价值比 `Join` 更低

所以它非常适合更激进的 TTL。


## 3. 当前发送点

### 3.1 `Join`

发送点：

- `comms/dht/src/actor.rs:543`

当前方式：

- `send_message_no_header(...broadcast(...))`


### 3.2 `Propagate Join`

发送点：

- `comms/dht/src/inbound/dht_handler/task.rs:261`

当前方式：

- `send_raw_no_wait(...)`


## 4. 推荐实现策略

与 `Ping/Pong` 一样，建议复用现有 DHT `expires` 字段。

也就是：

1. 发送时写入 `expires`
2. outbound 真正发送前检查是否过期
3. disconnect 后转 retry 前再次检查是否过期
4. 过期即丢弃

这样 `Join` / `Propagate Join` 不需要单独再发明一套清理逻辑。


## 5. 需要依赖的前置改动

这份方案默认你已经做了下面这些基础能力：

- `comms/dht/src/outbound/message_params.rs`
  - `FinalSendMessageParams` 增加 `expires`
  - `SendMessageParams::with_expires(...)`

- `comms/core/src/protocol/messaging/outbound.rs`
  - 增加 `is_expired_message(&OutboundMessage)`
  - 发送前检查过期
  - 转 retry 前检查过期

如果这些已经有了，那么 `Join` / `Propagate Join` 只需要在发送源头补 TTL 即可。


## 6. `Join` 的具体实现

文件：

- `comms/dht/src/actor.rs`

发送点：

- `comms/dht/src/actor.rs:543`

当前逻辑是构造一个 `broadcast(...)` 请求发送 `Join`。

建议改法：

- 构造 `SendMessageParams`
- 设置：
  - `broadcast(...)`
  - `with_dht_message_type(DhtMessageType::Join)`（如果当前逻辑已有则保持）
  - `with_expires(EpochTime::now() + 30s)`

示意：

```rust
let expires = EpochTime::now() + Duration::from_secs(30);

let params = SendMessageParams::new()
    .broadcast(vec![])
    .with_expires(expires)
    // 其他已有参数保持不变
    .finish();
```

注意：

- 如果这里当前已经有统一的 message validity window，优先复用现有值
- 如果没有，再直接写 `30s`


## 7. `Propagate Join` 的具体实现

文件：

- `comms/dht/src/inbound/dht_handler/task.rs`

发送点：

- `comms/dht/src/inbound/dht_handler/task.rs:261`

当前是：

- `send_raw_no_wait(...)`

建议改法：

- 在构造 `FinalSendMessageParams` 时补 `with_expires(...)`
- TTL 建议更短：
  - `10s` 或 `15s`

示意：

```rust
let expires = EpochTime::now() + Duration::from_secs(10);

let params = SendMessageParams::new()
    .with_expires(expires)
    // 其他 propagate/raw 相关参数保持不变
    .finish();
```

理由：

- `Propagate Join` 是传播副本
- 相比原始 `Join`，更应该尽快淘汰


## 8. 建议 TTL 数值

第一版建议：

- `Join`: `30s`
- `Propagate Join`: `10s`

更保守版本：

- `Join`: `60s`
- `Propagate Join`: `15s`

建议先从较短值开始压测和观测，因为这两类消息本来就不应该长期积压。


## 9. 会带来什么行为变化

### 9.1 正向收益

- 减少低价值旧消息长期占用 `OutboundMessage`
- 减少 retry queue 被旧 join 类消息占满
- 减少慢 peer 场景下的无效发送


### 9.2 可能副作用

- 极端慢网络下，部分 join 类消息可能在真正发出前就过期
- 某些 peer 会更依赖后续下一轮 `Join` / 其他传播路径获知节点信息

这个代价通常是可接受的，因为：

- 这类消息本来就更偏 gossip / best-effort
- 不值得为了保住每条传播副本而让内存持续增长


## 10. 测试建议

### 10.1 过期 `Join` 不会继续发送

思路：

- 构造带过期时间的 `Join`
- 让消息在队列中滞留到过期
- 断言：
  - 不会真正写到 sink
  - 会从 pending 中清掉


### 10.2 过期 `Propagate Join` 不进入 retry

思路：

- 构造即将过期的 `Propagate Join`
- 制造 disconnect
- 断言：
  - 不进入 retry queue
  - `retry_queue_messages` 不增加


## 11. 落地顺序建议

建议顺序：

1. 先完成 `Ping/Pong TTL`
2. 再补 `Propagate Join TTL`
3. 最后补 `Join TTL`

原因：

- `Ping/Pong` 时效最强，价值判断最清晰
- `Propagate Join` 次之
- `Join` 稍微更重要一些，放第三步更稳


## 12. 一句话结论

`Join` 和 `Propagate Join` 都适合按过期时间删除。

最小实现方式不是单独为它们再写一套清理逻辑，而是：

- 发送时写入 `expires`
- 复用 outbound 发送前 / retry 前的统一过期检查

第一版建议值：

- `Join`: `30s`
- `Propagate Join`: `10s`
