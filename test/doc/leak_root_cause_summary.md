# 内存持续上涨原因总结

本文档基于以下信息整理：

- `test/doc/leak.md` 及其中的 pprof 图片
- 当前 Tari outbound、broadcast、serialize、messaging 代码
- 出块节点持续上涨、普通节点相对正常的运行现象
- 当前工作区已经实施的消息过期清理和写阻塞保护

本文的目标是区分：

- 已被证据支持的结论
- 高概率根因
- 仍需要指标验证的推测
- 当前修改能够解决和不能解决的范围

## 1. 结论

当前内存持续上涨最可能不是传统意义上的“对象失去引用但无法释放”，而是：

```text
出块或同步产生广播消息
    ↓
一条广播消息按目标 peer 数量展开
    ↓
每个 peer 单独序列化并生成编码后的 Bytes
    ↓
部分 peer 的 messaging substream 写入长期阻塞或持续偏慢
    ↓
该 peer 的无界 outbound queue 无法及时消费
    ↓
Broadcast future、tower Oneshot、编码 buffer、后续消息持续被合法引用
    ↓
内存持续上涨
```

其中最危险的组合是：

1. 出块节点持续产生需要广播的消息。
2. 广播存在 fan-out，一条消息会复制为多个目标 peer 消息。
3. 每个目标 peer 都会产生独立的 DHT envelope 编码 buffer。
4. per-peer outbound queue 和 retry queue 使用无界 channel。
5. 某些 peer 连接没有真正断开，但不读取或无法及时读取 messaging substream。
6. `Forward` 长期停留在 `poll_ready` 或 `poll_flush`，导致后续队列不能被消费。

## 2. pprof 图片能够证明什么

`leak.md` 的 Outbound 图片显示：

- 前期 `BroadcastTask<S>::handle` 保留约 `326.50MB`
- 后期 `BroadcastTask<S>::handle` 保留约 `2.01GB`
- 调用链同时出现：
  - `tower::util::oneshot::Oneshot::poll`
  - `BroadcastTask<S>::handle`
  - `futures_util::stream::for_each::ForEach::poll`

这说明大量内存被 outbound 广播异步调用链持续持有。

pprof 图片支持以下判断：

- 广播任务没有及时完成。
- 大量发送 future 或其下游对象长期存活。
- 内存增长与 outbound 广播链路高度相关。

但 pprof 图片不能单独证明：

- `.unordered()` 是唯一根因。
- 所有 peer 都阻塞。
- 一定是 socket 写入永久阻塞。
- `prost::encode_to_vec` 自身存在泄漏。

`prost::encode_to_vec` 是分配热点，但更可能是 backlog 的放大结果：发送任务不结束，已经编码的 buffer 便不能释放。

## 3. 为什么出块节点更容易出现

出块节点和普通节点的主要差异是消息生产量。

出块后通常会产生需要传播的区块或链状态消息。即使平均约 5 分钟才出一个块，一条广播消息也会按 peer 数量展开：

```text
单条区块消息数量 × 目标 peer 数量 × 编码后的消息大小
```

如果某个 peer 长期无法消费，而该 peer 每次都被选为广播目标，那么每次出块都会向它的无界队列追加新消息。

因此：

- 正常 peer 可以继续收到区块。
- 网络上的其他节点可以看到新区块。
- 本节点仍然可以正常出块。
- 但少数异常 peer 的本地发送队列仍可能持续增长。

“网络已经收到区块”只能说明至少部分 peer 收到了，不代表所有目标 peer 都发送完成。

## 4. BroadcastTask 的放大作用

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

当前代码仍使用 `.unordered()`。

它会并发推进本轮广播中的目标 peer 发送 future。它的问题不是一定会永久泄漏，而是会放大下游慢或阻塞造成的内存保留：

- 目标 peer 越多，同时存在的发送 future 越多。
- 下游 service 越慢，future 存活时间越长。
- 新广播持续进入时，多个广播任务可能同时保留大量状态。

单独把 `.unordered()` 改成有限并发只能限制 broadcast 层的同时在途数量，不能解决某个 peer 的 per-peer queue 长期无法消费问题。

如果有限并发中的任务全部被慢 peer 阻塞，广播任务还可能整体停止推进。

## 5. Serialize 为什么显示大量内存

序列化位置：

```rust
let envelope = DhtEnvelope::new(dht_header, body.into());
let body = Bytes::from(envelope.to_encoded_bytes());
```

每个目标 peer 都会生成一份编码后的 DHT envelope。

即使原始消息体使用 `Bytes`，克隆成本较低，envelope 编码仍需要分配新的 buffer。

因此当消息积压时，内存中会同时保留：

- `DhtOutboundMessage`
- 编码后的 `Bytes`
- tower `Oneshot`
- 广播 future
- per-peer queue 中的 `OutboundMessage`
- retry queue 中的消息

所以 serialize/prost 是明显的内存分配来源，但根因更可能是发送链路无法及时释放这些编码结果。

## 6. 最可能的实际阻塞点

真正发送发生在 messaging outbound：

```rust
Forward::new(stream, sink).await
```

`Forward` 需要等待 sink：

```text
poll_ready
start_send
poll_flush
```

可能导致长期 Pending 的场景包括：

- 对端节点进程存活，但没有读取当前 messaging substream。
- 对端处理速度长期低于发送速度。
- Yamux 子流发送窗口耗尽。
- TCP send buffer 填满。
- 网络黑洞或防火墙静默丢包，连接暂时没有被操作系统判定为断开。
- 对端节点负载过高、磁盘或运行时卡顿。

这种情况下，TCP/Yamux 连接可能仍然显示为已连接，因此：

- `conn.on_disconnect()` 不会触发。
- `remote_stream.next()` 不会返回关闭。
- liveness Ping 也可能排在同一个 peer 队列后面，无法及时帮助判断。

## 7. 为什么内存会一直上涨而不是下降

只要消息生产速度长期大于消费速度，队列长度就不会下降：

```text
积压变化量 = 新增消息数量 - 成功发送或丢弃的消息数量
```

存在以下两种上涨模式。

### 7.1 永久阻塞

队头消息一直无法写出：

- 后续消息全部停留在该 peer 的无界队列。
- 新广播继续追加。
- 内存持续上涨。

### 7.2 长期吞吐不足

消息能够发送，但平均发送速度低于生产速度：

- 队列持续增长。
- 每条消息最终可能都能发送。
- 不会触发“单条消息永久阻塞”检测。

当前修改主要处理第一种情况，不能完全解决第二种情况。

## 8. RandomX 内存的判断

`leak.md` 中 RandomX 部分也显示明显的大内存占用。

RandomX VM 单个实例占用很大，并且 key 变化后可能创建新的 VM。但代码中存在最大 VM 数量限制，会移除旧实例。

因此 RandomX 更像：

- 大容量缓存
- 阶跃式上涨
- 有理论上限
- 旧实例释放后 RSS 不一定立刻归还操作系统

RandomX 可能贡献较高基础内存和阶段性增长，但不如 Outbound 路径符合“伴随持续出块不断上涨”的特征。

## 9. 当前已实施的缓解措施

当前工作区主要增加了以下保护。

### 9.1 保留消息过期时间

将 DHT `expires` 传递到 messaging 层的 `OutboundMessage`。

作用：

- 消息正常出队发送前，如果已经过期则直接丢弃。
- 连接结束后 drain 队列时，过期消息不再进入 retry。

限制：

- 队头永久阻塞时，后续消息无法出队，不能立即检查过期。

### 9.2 记录消息入队时间

每条 `OutboundMessage` 保存 `queued_at`。

作用：

- 区分消息业务 TTL 和本地队列积压时间。
- 判断当前队头消息是否已经在本地等待过久。

### 9.3 队头超过 3 小时后结束 Forward

`Forward` 在以下状态持续 Pending 时监控队头截止时间：

- `poll_ready`
- `poll_flush`
- `poll_close`

队头消息从入队开始超过 3 小时后，`Forward` 返回 `HeadMessageTooOld`。

### 9.4 断开原 PeerConnection

发生 `HeadMessageTooOld` 后：

- 根据 connection ID 确认仍然是发生阻塞的原连接。
- 主动断开整个 PeerConnection。
- 避免新 outbound handler 继续复用同一条异常 TCP/Yamux 连接。

### 9.5 清理超龄 retry 消息

旧 handler 退出后 drain per-peer queue：

- DHT 已过期消息直接丢弃。
- 本地排队超过 3 小时的消息直接丢弃。
- 仍有价值且未超龄的消息进入原有 retry 流程。

这样可以避免大量旧消息重新进入新连接，形成立即阻塞和反复重试。

## 10. 当前修改的影响范围

当前修改只影响本地发送行为，没有修改网络协议：

- 没有修改消息编码格式。
- 没有修改 DHT envelope 格式。
- 没有修改协议版本。
- 没有要求其他官方节点升级。

因此只有本节点升级时，仍然可以和官方版本节点通信。

触发 3 小时阻塞保护时可能产生以下影响：

- 断开该 peer 的全部 Yamux 子流，包括正在使用的 RPC 或同步子流。
- 当前已经进入 `Forward`、但尚未真正 flush 完成的消息可能丢失。
- 未超过 3 小时的排队消息可能 retry，并存在重复发送可能。
- 旧区块传播消息可能被丢弃，但 peer 重新连接后仍可通过区块同步获得区块。

正常能够及时消费消息的 peer 不会触发该逻辑。

## 11. 当前方案仍不能解决的问题

### 11.1 3 小时窗口内仍可能增长

per-peer queue 仍然是无界队列。阻塞后的 3 小时内，新消息仍会继续进入并占用内存。

### 11.2 慢但持续有进展的 peer

如果 peer 一直能够发送少量消息，但生产速度始终更快，队列仍会增长，而且可能不会触发当前队头阻塞保护。

### 11.3 已进入 sink 的消息状态不完整

消息进入 `Forward` 后只保留编码后的 body 和 deadline。当前代码仍会在真正 flush 完成前调用 `reply_success()`。

因此发生超时时，上层可能已经收到成功结果，但消息实际未完全发出。

### 11.4 无界入口和 retry queue

DHT outbound 入口、per-peer queue 和 retry queue 仍有无界增长风险。当前方案是止血措施，不是完整背压方案。

## 12. 根因可信度排序

### 高可信

1. Outbound 广播异步链路长期持有大量内存。
2. 广播 fan-out 和 per-peer 编码显著放大内存。
3. 无界 per-peer queue 在慢 peer 场景下能够持续积压。
4. 出块节点因为持续产生广播消息，更容易暴露该问题。

### 较高可信

1. 少数 peer 的 messaging substream 长期写不出去或消费极慢。
2. `Forward` 长期 Pending，导致该 peer 后续消息无法出队。
3. `prost::encode_to_vec` 增长是积压消息长期存活的结果，而不是独立泄漏。

### 仍需验证

1. 实际线上是否存在队头连续阻塞超过 3 小时。
2. 是永久阻塞为主，还是长期吞吐不足为主。
3. 积压主要集中在少数 peer，还是广泛分布于多个 peer。
4. retry queue 是否也是主要积压位置。

## 13. 建议重点观测

上线当前修改后，重点观察：

- 是否出现：

```text
Disconnected peer ... because the outbound queue head was blocked
```

- 触发是否集中在少数固定 peer。
- 触发后 RSS 是否停止上涨或明显下降。
- 触发后节点出块、同步和 peer 数量是否恢复正常。
- outbound enqueue 与 dequeue 差值是否持续扩大。
- retry queue 是否持续增长。
- 每个 peer 的队列长度和最老消息年龄。

判断方式：

- 如果触发断开后内存水位稳定，说明阻塞 peer 是主要根因。
- 如果没有触发断开但内存仍持续上涨，更可能是长期吞吐不足或其他无界队列积压。
- 如果触发频繁且集中于正常 peer，需要检查底层 Yamux/TCP 或降低广播压力，而不是继续依赖断开重试。

## 14. 最终判断

综合 `leak.md` 的 pprof 证据、出块节点特征和当前代码，当前最合理的根因判断是：

> 出块和同步产生的广播消息经过 fan-out 和逐 peer 序列化后，进入无界发送链路。部分 peer 的 messaging substream 长期阻塞或消费速度不足，导致广播 future、编码后的 buffer 和 per-peer 队列消息长期被持有。新出块继续产生新广播，因此内存持续上涨。

RandomX 是明显的大内存来源，但有缓存数量上限，更可能造成基础内存较高和阶段性上涨，不是当前持续上涨的首要怀疑对象。

当前实施的“队头积压超过 3 小时后断开原连接，并丢弃过期或超龄消息”属于最小侵入的止血方案。它能够处理永久阻塞，但不能替代完整的队列背压、慢 peer 隔离和发送完成语义修正。
