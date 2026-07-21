# 挖矿节点可能存在的 outbound 内存持续增长问题

## 问题概述

我们在一个持续出块的挖矿的正式网基础节点上观察到了内存持续上涨的问题，大致情况如下图所示，这是内存可用图片，由此可见，是用户

节点本身仍然可以正常运行：
- 矿工提交的区块可以成功被节点接受；
- 节点可以持续正常出块；
- 网络中的其他节点可以收到新区块；

但是，从 heap profile 来看，存活内存中有很大一部分持续归因到 outbound broadcast 路径，并且会随着运行时间显著增长。

我们想确认，这是否可能是由于某些慢 peer 或写出停滞的 peer 导致 outbound 消息长期滞留而引起的。

## Profiling 证据

较早时间点的 profile：

![Earlier outbound profile](img_6.png)

其中归因到：

```text
tari_comms_dht::outbound::broadcast::BroadcastTask<S>::handle
```

的存活内存大约为 `326.50MB`。

较晚时间点的 profile：

![Later outbound profile](img_7.png)

同一条 outbound 调用路径增长到了大约 `2.01GB`。

同时还可以看到大约 `1.94GB` 归因到：

```text
futures_util::stream::for_each::ForEach::poll
```

此外，我们也观察到由 Protobuf 编码路径产生的存活分配持续增长。

我们理解这些 profile 只说明“内存最初在哪里分配”，并不一定说明这些对象当前仍然由 `BroadcastTask` 自身持有；这些编码后的消息也可能已经移动到了更下游的队列或网络写出 future 中。

## 相关的区块传播路径

当矿工提交一个有效区块后，节点会将其加入本地链并传播 `NewBlock` 消息：

```text
gRPC submit_block
    → LocalNodeCommsInterface::submit_block
    → add block
    → OutboundNodeCommsInterface::propagate_block
    → handle_outbound_block
    → OutboundMessageRequester::propagate
    → BroadcastTask::handle
    → SerializeMiddleware::call
    → per-peer outbound messaging queue
    → Forward
    → Yamux/TCP
```

当前 DHT 默认的 `propagation_factor` 是 `20`，因此一次成功出块可能会为多个 peer 生成传播消息。

并且每个目标 peer 都会独立编码一份 DHT envelope：

```rust
let body = Bytes::from(envelope.to_encoded_bytes());
```

## 怀疑点

### 1. broadcast fan-out

`BroadcastTask::handle` 会并发处理本轮传播选中的所有消息：

```rust
self.service
    .call_all(stream::iter(messages))
    .unordered()
    .filter_map(|result| future::ready(result.err()))
    .for_each(...)
    .await;
```

这意味着一次传播会把面向多个 peer 的消息同时推进到后续链路。

### 2. 多处无界队列

当前 outbound 路径中存在多处无界队列，包括：

- base node 的 outbound block queue；
- DHT outbound request queue；
- 每个 peer 自己的 outbound messaging queue；
- messaging retry queue。

尤其是每个 peer 都有自己的无界 outbound queue：

```rust
let (msg_tx, msg_rx) = mpsc::unbounded_channel();
```

### 3. 慢 peer / 写出停滞

每个 peer 的消息最终通过：

```rust
Forward::new(stream, sink).await
```

写入到底层网络。

`Forward` 依赖 `poll_ready` 和 `poll_flush` 推进 sink。

我们当前的怀疑是：某些连接可能保持“已连接”状态，但其 messaging substream 在 `poll_ready` 或 `poll_flush` 上长期无法取得足够进展。这样的话，新区块传播消息仍然会继续追加到这些 peer 的 outbound queue 中。

这可以解释为什么：

- 节点本身和大多数 peer 依然工作正常；
- 网络上依然能看到新区块；
- 只有一个或少数慢 peer 就可能持续积压消息；
- 编码后的 outbound buffer 持续存活，最终表现为 RSS 持续上涨。

## 当前分析结论

基于 heap profile 和当前 outbound 实现，我们认为较大概率的情况是：

**编码后的 outbound 消息在一个或少数 peer 的 per-peer messaging queue 中持续累积。**

可能的过程如下：

1. 一次成功出块触发 `NewBlock` 传播；
2. DHT propagation 选择多个 peer，默认 `propagation_factor = 20`；
3. 每个被选中的 peer 都会独立编码一份 DHT envelope buffer；
4. 编码后的消息进入该 peer 的无界 outbound queue；
5. 如果某个 peer 的 messaging substream 在 `poll_ready` 或 `poll_flush` 上持续无法推进，则该 peer 的 outbound handler 不会退出；
6. 后续新区块消息继续被追加到该 peer 的 queue；
7. 这些编码 buffer 因此长期存活，导致 RSS 持续上涨，并在 pprof 中表现为 `BroadcastTask::handle` 和 Protobuf 编码路径的存活内存不断增长。

这个问题不要求所有 peer 都阻塞。只要有一个或少数 peer 出现长期慢写，其他 peer 仍然可以正常收到块，因此可以同时出现“节点正常出块、网络正常收块、但本地内存持续上涨”的现象。

此外，我们注意到：

- 现有的 10 秒 outbound pipeline timeout，看起来并不能清理那些**已经序列化并进入 per-peer messaging queue** 的消息；
- `reply_success()` 调用时机更像是“消息已从 queue 取出并交给 `Forward`”，而不是“消息已经真正 flush 到网络”；
- 当前 per-peer queue 和 retry queue 都是无界的；
- 在 `Forward` 这一层我们暂未看到明确的 write-stall timeout 机制。

## 希望确认的问题

想请项目方帮忙确认以下几点：

1. 是否存在已知问题：peer 连接仍然保持 active，但其 messaging substream 长期阻塞在 `poll_ready` 或 `poll_flush`？
2. 如果 peer 保持连接但不继续消费消息，其 per-peer outbound queue 是否可能无限增长？
3. `Forward` 或底层 Yamux/TCP 是否存在 write-stall timeout？
4. `SendMessageResponse::Queued` 或 `reply_success()` 的语义，是否只是表示消息已入队/出队，而不是已经真正写入网络？
5. 10 秒的 outbound pipeline timeout，是否能够清理那些已经进入 per-peer messaging queue 的消息？
6. 当前是否已有办法观测单个 peer 的 outbound queue 长度、最老消息年龄或者实际写出进度？
7. 项目方是否在挖矿节点或频繁传播 `NewBlock` 的节点上观察到类似的内存增长模式？
8. per-peer 无界队列和 retry 无界队列，是否是面向长期生产环境运行的预期设计？

## 补充背景

- 这个问题在挖矿节点上比普通节点更明显；
- 当前大约每 5 分钟会成功产出一个区块；
- 网络中能看到新区块，只能说明“至少有部分 peer 收到了块”，不能说明“本轮所有被选中的 peer 都完成了发送”；
- 我们目前将 `BroadcastTask` 和 Protobuf 编码视为“分配来源”，但最终的持有位置很可能在更下游。

如果项目方能够提供关于 outbound queue 预期行为、slow peer 处理机制、或者更适合定位 retention 点的指标/日志建议，会非常有帮助。
