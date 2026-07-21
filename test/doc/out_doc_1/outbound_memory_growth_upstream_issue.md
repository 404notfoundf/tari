# 正式网挖矿节点 outbound 相关内存持续增长问题

## 问题概述

我们在一个持续出块的正式网挖矿基础节点上观察到了内存持续上涨的问题。

当前使用的版本是：`release 5.3.0`。

具体现象为：节点运行期间内存占用持续上涨，长时间没有明显回落；与此同时，节点仍可正常联网、正常接收矿工提交的区块，并持续正常出块。
结合当前现象和 profile 结果，我们认为该问题与 outbound 路径相关，但目前还无法确定究竟是哪个环节导致了内存持续增长。

整体趋势大致如下：随着节点持续运行，可用内存逐步减少，且长时间没有明显回升；重启节点后，可用内存会立即明显恢复。

![Overall memory trend](img.png)

## Profiling 证据

较早时间点的 profile：

![Earlier outbound profile](img_6.png)

其中归因到：

```text
tari_comms_dht::outbound::broadcast::BroadcastTask<S>::handle
```

的存活内存大约为 `326.50MB`。

几天之后的 profile：

![Later outbound profile](img_7.png)

同一条 outbound 调用路径增长到了大约 `2.01GB`。

同时还可以看到大约 `1.94GB` 归因到：

```text
futures_util::stream::for_each::ForEach::poll
```

此外，我们也观察到由 Protobuf 编码路径产生的存活分配持续增长。

我们理解这些 profile 只能说明“内存最初在哪里分配”，并不一定说明这些对象当前仍然由 `BroadcastTask` 自身持有；这些编码后的消息也可能已经移动到了更下游的队列或网络写出 future 中。

## 为什么怀疑 outbound 路径

我们集成了 `jemalloc_pprof` 来分析内存分配情况。从 heap profile 来看，outbound broadcast 相关调用路径对应的存活内存占比持续增大，并且会随着运行时间明显增长。

结合当前现象，我们目前怀疑问题主要出现在 outbound 路径，但还无法完全确认最终根因，因此希望能结合这部分实现帮忙判断这更接近哪一类问题。

## 当前初步判断

以下内容主要是结合现象、profile、代码路径以及 AI 辅助分析得到的初步判断，具体根因目前仍无法完全确定。

从当前信息看，我们更倾向于认为，问题可能出现在 outbound 路径后半段：广播产生的消息在后续无界队列或写出阶段发生滞留，而某个或少数 peer 的写出路径长期无法推进时，这种滞留会被进一步放大，最终表现为相关内存持续存活和 RSS 持续上涨。

这并不要求所有 peer 都阻塞。只要少数 peer 长期慢写，其他 peer 仍然可以正常收到块，因此就可能同时出现“节点正常出块、网络正常收块，但本地内存持续上涨”的现象。

另外，从当前实现和现象来看，`BroadcastTask::handle` 中的 fan-out 可能放大单次传播产生的在途消息数量，而现有的 outbound pipeline timeout 看起来又未必能够清理已经进入 per-peer messaging queue 的消息；与此同时，当前 per-peer queue 和 retry queue 都是无界的，`reply_success()` 与 `Forward` 这一层的行为也暂时不足以直接说明消息已经真正写出并被及时释放。

## 希望项目方协助分析问题原因

基于当前的 profile、运行现象和代码路径，我们目前只能判断问题大概率与 outbound 路径相关，但还无法确定最终根因。

希望项目方能够结合当前实现协助分析一下，这类内存持续增长问题最可能的原因是什么，是否存在比较明显的风险点或已知问题。
