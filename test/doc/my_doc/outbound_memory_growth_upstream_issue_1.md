# 正式网挖矿节点 outbound 相关内存持续增长问题

## 问题概述

我们在一个持续出块的正式网挖矿基础节点上观察到了内存持续上涨的问题。

当前使用的版本是：`release 5.3.0`。

具体现象为：节点运行期间内存占用持续上涨，长时间没有明显回落；同时节点仍可正常出块、正常联网，并正常接收矿工提交的区块。
结合当前现象和 profile 结果，我们认为该问题与 outbound 路径相关，但目前还无法确定究竟是哪个环节导致了内存持续增长。

整体趋势大致如下：随着节点持续运行，可用内存逐步减少，且长时间没有明显回升；重启节点后，可用内存会立即明显恢复。

![Overall memory trend](img.png)

节点本身仍然可以正常运行：
- 矿工提交的区块可以成功被节点接受；
- 节点可以持续正常出块；

## Profiling 证据

较早时间点的 profile：

![Earlier outbound profile](img_6.png)

其中归因到：

```text
tari_comms_dht::outbound::broadcast::BroadcastTask<S>::handle
```

的存活内存大约为 `326.50MB`。

大概几天之后的 profile：

![Later outbound profile](img_7.png)

同一条 outbound 调用路径增长到了大约 `2.01GB`。

同时还可以看到大约 `1.94GB` 归因到：

```text
futures_util::stream::for_each::ForEach::poll
```

此外，我们也观察到由 Protobuf 编码路径产生的存活分配持续增长。

我们理解这些 profile 只能说明“内存最初在哪里分配”，并不一定说明这些对象当前仍然由 `BroadcastTask` 自身持有；这些编码后的消息也可能已经移动到了更下游的队列或网络写出 future 中。

## 为什么怀疑 outbound

我们集成了 `jemalloc_pprof` 来分析内存分配情况。从 heap profile 来看，outbound broadcast 相关调用路径对应的存活内存占比持续增大，并且会随着运行时间明显增长。

结合当前现象，我们目前怀疑问题主要出现在 outbound 路径，但还无法完全确认最终根因，因此希望能结合这部分实现帮忙判断这更接近哪一类问题。

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

这条路径是我们怀疑 outbound 的主要依据之一。

## 当前怀疑点

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

这意味着一次传播会将面向多个 peer 的消息同时推进到后续链路。

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

我们当前的怀疑是：某些连接可能保持“已连接”状态，但其 messaging substream 在 `poll_ready` 或 `poll_flush` 上长期无法取得足够进展。在这种情况下，新区块传播消息仍然会继续追加到这些 peer 的 outbound queue 中。

这可以解释为什么：

- 节点本身和大多数 peer 依然工作正常；
- 网络上依然能看到新区块；
- 只有一个或少数慢 peer 就可能持续积压消息；
- 编码后的 outbound buffer 持续存活，最终表现为内存持续上涨。

## 当前分析结论

基于目前的 heap profile、代码路径和运行现象，我们当前更倾向于认为：

**问题可能出现在 outbound 路径后半段的消息滞留、队列积压，或写出长期无法推进的阶段，但目前还无法确认最终的 retention 点。**

可能的过程如下：

1. 一次成功出块触发 `NewBlock` 传播；
2. DHT propagation 选择多个 peer，默认 `propagation_factor = 20`；
3. 每个被选中的 peer 都会独立编码一份 DHT envelope buffer；
4. 编码后的消息进入该 peer 的无界 outbound queue；
5. 如果某个 peer 的 messaging substream 在 `poll_ready` 或 `poll_flush` 上持续无法推进，则该 peer 的 outbound handler 不会退出；
6. 后续新区块消息继续被追加到该 peer 的 queue；
7. 这些编码 buffer 因此长期存活，导致 RSS 持续上涨，并在 pprof 中表现为 `BroadcastTask::handle` 和 Protobuf 编码路径的存活内存不断增长。

这个问题不要求所有 peer 都阻塞。只要有一个或少数 peer 出现长期慢写，其他 peer 仍然可以正常收到块，因此就可能同时出现“节点正常出块、网络正常收块、但本地内存持续上涨”的现象。

此外，我们注意到：

- 现有的 10 秒 outbound pipeline timeout，看起来并不能清理那些**已经序列化并进入 per-peer messaging queue** 的消息；
- `reply_success()` 调用时机更像是“消息已从 queue 取出并交给 `Forward`”，而不是“消息已经真正 flush 到网络”；
- 当前 per-peer queue 和 retry queue 都是无界的；
- 在 `Forward` 这一层我们暂未看到明确的 write-stall timeout 机制。

## 想请项目方帮忙判断

基于当前 profile、代码路径和现象，想请项目方帮忙判断以下几点：

1. 我们当前这条分析方向是否合理：`NewBlock` 广播后，消息可能在一个或少数 peer 的 outbound 路径中长期滞留，从而导致内存持续增长。
2. 从当前实现来看，这类问题最可能卡在哪一层：`BroadcastTask`、per-peer queue、retry queue，还是 `Forward` / Yamux / TCP 写出阶段。
3. 如果这是你们见过的类似问题，通常建议优先从哪一层排查，或者是否有更推荐的处理方向。
