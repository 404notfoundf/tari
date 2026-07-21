# leak.md 再分析：内存持续上涨原因整理

本文档基于 `test/doc/leak.md` 和其中图片重新梳理内存上涨原因。

结论先行：

- `RandomXFactory::create` 是明显的大内存来源，但更像有上限缓存增长。
- `Outbound / BroadcastTask::handle` 更像造成内存持续上涨的主因。
- 真正危险点不是单个对象不释放，而是发送链路里大量 future、队列、序列化 buffer 长时间被持有。


## 1. RandomXFactory 部分

`leak.md` 中 RandomX 相关图片显示：

- 前：`RandomXFactory::create` 约 `768MB`
- 后：`RandomXFactory::create` 约 `1.25GB`

相关代码：

- `base_layer/core/src/proof_of_work/randomx_factory.rs`

代码里明确说明：

```rust
// Note: Memory required per VM in light mode is 256MB
```

`RandomXFactoryInner` 内部维护：

```rust
vms: HashMap<Vec<u8>, (Instant, RandomXVMInstance)>
max_vms: usize
```

每次不同 key 进来时，可能创建新的 `RandomXVMInstance`。

### 1.1 为什么会涨

RandomX VM/cache 本身占用很大。key 变化后，会创建新的 VM 实例并缓存。

所以它会带来明显的 RSS 增长。

### 1.2 为什么它不像无界泄露

代码里有 `max_vms` 限制：

```rust
if self.vms.len() >= self.max_vms {
    // remove oldest key
}
```

也就是说：

- 它可能出现阶跃式上涨
- 可能在 key 变化时突然多出几百 MB
- 但理论上有上限

因此 RandomX 更像“大块缓存水位升高”，不是最符合“持续无限上涨”的点。


## 2. Outbound 部分

`leak.md` 中 Outbound 图片显示：

- 前：`BroadcastTask::handle` 约 `326MB`
- 后：`BroadcastTask::handle` 约 `2.01GB`

调用链：

1. `comms/dht/src/outbound.rs`
2. `comms/dht/src/outbound/broadcast.rs`
3. `comms/dht/src/outbound/serialize.rs`

图片中还出现：

- `tower::util::Oneshot`
- `Future::poll`
- `BroadcastTask<S>::handle`
- `ForEach`

这说明增长主要出现在异步发送 future 链路中。


## 3. BroadcastTask::handle 的问题

核心位置：

```rust
self.service
    .call_all(stream::iter(messages))
    .unordered()
    .filter_map(|result| future::ready(result.err()))
    .for_each(|err| {
        warn!(target: LOG_TARGET, "Error when sending broadcast messages: {err}");
        future::ready(())
    })
    .await;
```

原始问题是：

- `.unordered()` 会无界推进所有发送 future
- 如果下游 service 慢，future 会长期悬挂
- 新广播继续叠加，内存会持续增长

现在已改成：

```rust
.buffer_unordered(MAX_BROADCAST_IN_FLIGHT)
```

这能限制 broadcast 这一层的同时在途 future 数量。

但要注意：

- 它只能限制这一层无界并发
- 如果下游 64 个 future 全卡住，当前 broadcast 仍会等待
- 它不能单独解决真正 socket 写出卡死的问题


## 4. fan-out 放大

在 `broadcast.rs` 中，一条上游消息会根据 selected peers 生成多条 `DhtOutboundMessage`：

```rust
let messages = selected_peers.into_iter().map(|node_id| {
    DhtOutboundMessage { ... }
});
```

这意味着：

- 一条业务消息不是只占一份内存
- 会按目标 peer 数量放大
- 每个 peer 都有自己的 message、reply channel、send state

如果：

- 广播频繁
- peer 数量多
- 下游发送慢

那么 backlog 会被 fan-out 乘法放大。


## 5. serialize.rs 的重复分配

在 `serialize.rs` 中：

```rust
let envelope = DhtEnvelope::new(dht_header, body.into());
let body = Bytes::from(envelope.to_encoded_bytes());
```

这里会把 DHT envelope 编码成新的 buffer。

这对应 `leak.md` 中 `prost::Message::encode_to_vec` 增长。

关键点：

- 上层 body 即使是 `Bytes`，clone 很便宜
- 但 DHT envelope 序列化会重新分配
- 每个目标 peer 都可能产生独立编码 buffer
- 如果 future 卡住，这些 buffer 会被长期持有

所以 `prost::Message::encode_to_vec` 增长不是独立问题，而是 Outbound backlog 的症状之一。


## 6. 无界队列仍然是风险点

即使 broadcast 限并发，后面仍有多个无界队列。

### 6.1 DHT outbound requester 入口

位置：

```rust
self.sender.send(DhtOutboundRequest::SendMessage(...))?
```

这里使用 unbounded sender。上游可以持续把消息塞进 DHT outbound。

### 6.2 per-peer outbound queue

位置：

```rust
let (msg_tx, msg_rx) = mpsc::unbounded_channel();
```

每个 peer 都有一个 outbound queue。慢 peer 会导致该 peer 队列持续增长。

### 6.3 retry queue

位置：

```rust
let (retry_queue_tx, retry_queue_rx) = mpsc::unbounded_channel();
```

连接断开后，未发送消息会进入 retry queue。如果断连频繁或下游一直慢，retry queue 也会累积。


## 7. retry 的作用和风险

retry 是 comms 层的连接中断补发机制。

流程：

1. outbound handler 正在给某个 peer 发消息
2. 连接或 substream 结束
3. 当前 peer queue 中剩余消息被 drain 出来
4. 这些消息进入 retry queue
5. `MessagingProtocol` 再次调用 `send_message`
6. 如有需要，重新 spawn outbound handler 并重新拨号

它的价值是提高连接抖动场景下的发送成功率。

风险是：

- retry queue 是无界的
- 如果 peer 长期不可用或发送长期慢，retry 会延长消息生命周期
- 旧消息会继续占内存

目前已加过期清理：

- 发送前检查 `is_expired`
- 转 retry 前检查 `is_expired`

但这只能清理已经到达这些检查点的消息。


## 8. 如果卡在 Forward::new，TTL 不一定及时生效

关键位置：

```rust
super::forward::Forward::new(stream, sink.sink_map_err(Into::into)).await?;
```

如果这里卡住：

- `Forward::new` 不返回
- 代码不会走到后面的 `messages_rx.close()`
- 也不会 drain queue
- retry 前的过期检查不会执行

发送前检查只能处理：

- 消息从 `messages_rx` 取出来时已经过期

不能处理：

- 消息取出来时未过期
- 之后卡在 sink 写出过程中
- 卡住期间才过期

因此，如果真正阻塞点在 socket/sink 写出，TTL 不是完整解法。


## 9. 当前最可能造成持续上涨的原因排序

### 9.1 Broadcast 无界在途 future

原始 `.unordered()` 会无界推进 future。

这是 `BroadcastTask::handle` 增长的直接原因之一。

### 9.2 广播 fan-out

一条上游消息会变成多条 per-peer 消息。

peer 越多，内存放大越明显。

### 9.3 Protobuf envelope 重复编码

每个目标 peer 都会经过 `to_encoded_bytes()`，产生新的 buffer。

这些 buffer 会跟随 future / queue 长时间存活。

### 9.4 per-peer queue 无界

慢 peer 会导致单个 peer 队列持续增长。

### 9.5 retry queue 无界

断连或发送失败时，消息可能进入 retry queue，生命周期进一步延长。

### 9.6 DHT outbound 入口无界

上游可以持续生产消息，入口不形成背压。

### 9.7 Forward::new 卡住

如果真正写 socket 卡住，后续 retry/过期清理都不会及时执行。

### 9.8 RandomX cache

RandomX 是大内存来源，但理论上有上限，更像阶跃增长。


## 10. 当前已经做过的缓解

### 10.1 broadcast 限并发

已把：

```rust
.unordered()
```

改为：

```rust
.buffer_unordered(MAX_BROADCAST_IN_FLIGHT)
```

作用：

- 限制 broadcast 层同时在途 future 数量
- 防止单次 broadcast 无界放大

### 10.2 消息 TTL

已给以下消息接入 TTL：

- `Ping`
- `Pong`
- `Join`
- `Propagate Join`
- `Discovery`

作用：

- 过期后不再真正发送
- 过期后不再进入 retry

### 10.3 Prometheus 指标

已增加观测指标：

- `outbound_queue_enqueue_count`
- `outbound_queue_dequeue_count`
- `outbound_pending_messages`
- `retry_queue_messages`
- `active_outbound_queues`

作用：

- 判断是不是 outbound backlog 持续增长
- 判断 retry 是否持续积压


## 11. 仍然需要继续验证的点

最关键的是确认是否卡在：

```rust
Forward::new(stream, sink)
```

如果这里长期不返回，说明问题更接近：

- 某些 peer 写不出去
- socket/substream backpressure
- peer 不读数据
- yamux 层阻塞

这时单靠 TTL 不够，需要处理阻塞发送。


## 12. 后续建议

### 12.1 先加阻塞观测

建议观测：

- 每个 peer 最近成功发送时间
- 单条消息发送耗时
- `Forward::new` 持续运行时间
- 是否长期没有 dequeue

这是低风险改动，不改变语义。

### 12.2 对慢 peer 做断开或降级

如果某个 peer 长时间写不出去：

- 主动断开该 peer
- 让 outbound handler 退出
- 触发 queue drain
- 让过期消息被清理
- 非过期消息重新建连接

### 12.3 per-peer queue bounded

这是更根本的保护。

慢 peer 不应拥有无限队列。

### 12.4 retry queue bounded

retry queue 也不应无限增长。

### 12.5 按消息类型做策略

不同消息应走不同策略：

- 强时效消息：TTL 到期丢弃
- best-effort 广播：队列满时失败或丢弃
- 重要请求/响应：背压、超时失败或显式错误


## 13. 总结

`leak.md` 中真正需要重点关注的是 Outbound。

RandomX 会消耗大量内存，但有缓存上限。

Outbound 的持续上涨更可能来自：

- broadcast 无界或高并发 future
- fan-out 放大
- protobuf 重复编码
- per-peer queue 无界
- retry queue 无界
- socket/sink 写出阻塞

当前已经做的 TTL 和 broadcast 限并发能缓解一部分问题，但如果 `Forward::new` 卡住，还需要继续处理慢 peer / 写出阻塞。
