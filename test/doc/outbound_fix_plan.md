`OutboundMessage` 堆积问题的最小改动修复方案

本文档用于整理：

1. 如果最终确认是 `OutboundMessage` 发送链路导致内存持续上涨
2. 在“不大改架构”的前提下，应该如何优先修改
3. 每种修改的收益、风险和建议顺序


## 1. 问题背景

当前更可疑的不是 Rust 经典意义上的对象泄露，而是：

- outbound 广播 fan-out 无界并发
- per-peer outbound queue 无界
- retry queue 无界
- 下游发送慢时，`OutboundMessage` 生命周期被拉长
- 结果表现为 RSS 持续上涨，像“泄露”

也就是说，根因更像是：

- 背压不足
- 限流不足
- 队列没有上限

而不是 `drop` 失效。


## 2. 修改目标

如果要求“改动小、风险低、容易验证”，目标不是一次性重构整条链路，而是：

1. 把无界堆积改成有界堆积
2. 把无限等待改成有限等待
3. 尽量只改局部热点位置
4. 保持现有接口和大部分行为不变


## 3. 最推荐的修改顺序

建议顺序如下：

1. 先限制 `broadcast` 内部 fan-out 并发
2. 再把 per-peer outbound queue 改为 bounded
3. 再给单条消息发送增加超时
4. 最后限制 retry queue

原因：

- 第 1 步改动最小，但收益往往最大
- 第 2 步最接近真正的堆积点
- 第 3 步用于避免慢 peer 长时间拖住消息
- 第 4 步用于兜底，防止断连后无限回灌


## 4. 第一优先级：限制 broadcast 内部并发

代码位置：

- `comms/dht/src/outbound/broadcast.rs`

当前逻辑里最危险的是：

```rust
self.service
    .call_all(stream::iter(messages))
    .unordered()
```

问题：

- `.unordered()` 没有并发上限
- 一次广播给很多 peer 时，会同时驱动大量发送 future
- 下游慢时，大量消息同时在途

最小改法：

- 把 `.unordered()` 改成有限并发

可选写法：

```rust
stream::iter(messages)
    .map(|msg| self.service.call(msg))
    .buffer_unordered(MAX_BROADCAST_IN_FLIGHT)
```

或者：

```rust
.for_each_concurrent(MAX_BROADCAST_IN_FLIGHT, ...)
```

建议初始值：

```rust
const MAX_BROADCAST_IN_FLIGHT: usize = 32;
```

收益：

- 改动很小
- 不需要改外围接口
- 可以显著减少大量在途 `DhtOutboundMessage`

风险：

- 广播峰值吞吐可能下降
- 但通常比无界堆积更可接受

结论：

- 如果只能改一处，优先改这里


## 5. 第二优先级：把 per-peer outbound queue 改成 bounded

代码位置：

- `comms/core/src/protocol/messaging/protocol.rs`

当前逻辑：

- `active_queues: HashMap<NodeId, mpsc::UnboundedSender<OutboundMessage>>`
- `let (msg_tx, msg_rx) = mpsc::unbounded_channel();`

问题：

- 某个 peer 发送慢时，会持续积压在该 peer 的发送队列中
- 即使别的链路做了限流，慢 peer 仍然可以把内存拖高

最小改法：

- 把 `mpsc::unbounded_channel()` 改成 `mpsc::channel(PER_PEER_QUEUE_SIZE)`

例如：

```rust
const PER_PEER_QUEUE_SIZE: usize = 64;
```

然后把：

```rust
sender.send(out_msg)
```

改成：

```rust
sender.try_send(out_msg)
```

或者：

```rust
sender.send(out_msg).await
```

两种方式的区别：

- `try_send`
  - 队列满时立刻失败
  - 改动相对小
  - 不会把背压一路向上传递

- `send().await`
  - 队列满时等待
  - 能形成真实背压
  - 但对调用链行为影响更大

如果希望改动更小，建议先用：

- `try_send + 失败计数/日志`

收益：

- 直接限制单个慢 peer 的内存占用
- 非常贴近真实堆积点

风险：

- 队列满时会开始丢消息或返回错误
- 需要业务接受这种背压策略


## 6. 第三优先级：给发送加超时

代码位置：

- `comms/core/src/protocol/messaging/outbound.rs`

问题：

- 即使不无限排队，单条发送如果一直卡住，仍会长期占着资源

最小改法：

- 在真正发送路径外层包一层 `tokio::time::timeout`

例如思路：

```rust
tokio::time::timeout(SEND_TIMEOUT, forward_future).await
```

建议初始值：

```rust
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
```

收益：

- 防止少数极慢连接长期占住发送资源
- 让堆积更快进入可观测失败或 retry 路径

风险：

- 网络波动大时，可能误伤慢但最终可成功的发送

适用场景：

- 已经确认存在慢 peer 或长时间阻塞


## 7. 第四优先级：限制 retry queue

代码位置：

- `comms/core/src/protocol/messaging/protocol.rs`

当前逻辑：

- `let (retry_queue_tx, retry_queue_rx) = mpsc::unbounded_channel();`

问题：

- 断连后剩余消息会被回灌到 retry queue
- 如果 retry queue 无上限，仍可能继续涨内存

最小改法：

- 改成 `mpsc::channel(RETRY_QUEUE_SIZE)`

建议初始值：

```rust
const RETRY_QUEUE_SIZE: usize = 256;
```

策略建议：

- 队列满时直接丢弃并打日志/metrics
- 不建议无限等待

收益：

- 防止断连/抖动场景下的长期积压

风险：

- retry 消息可能被丢弃


## 8. 如果只能做一版最小修复

如果只允许做一版改动，建议只改两处：

1. `broadcast.rs` 中把无界并发改成有限并发
2. `protocol.rs` 中把 per-peer queue 改成 bounded

这是“收益最大 / 改动最小 / 最容易验证”的组合。


## 9. 不建议第一时间做的事情

如果目标是低风险小改动，先不要直接做这些：

- 重构 `OutboundMessage` 结构
- 整体重写 outbound requester
- 全链路改成复杂优先级调度
- 给每类消息设计不同的 QoS
- 一次性把所有无界队列全部改掉且联动修改业务行为

这些都可能是后续优化方向，但不适合第一刀。


## 10. 如何判断修改是否有效

可以结合本次新增的 Prometheus 指标判断：

- `comms::messaging::outbound_pending_messages`
- `comms::messaging::retry_queue_messages`
- `comms::messaging::outbound_queue_enqueue_count`
- `comms::messaging::outbound_queue_dequeue_count`

判断方法：

### 10.1 修改前

如果线上或压测时出现：

- `outbound_pending_messages` 持续上涨不回落
- `enqueue_count - dequeue_count` 持续扩大
- `retry_queue_messages` 在断连时越来越高

说明发送链路在堆积。

### 10.2 修改后

理想表现应该是：

- `outbound_pending_messages` 在一个平台附近波动，而不是无限上涨
- `retry_queue_messages` 有峰值但不会长期无界增长
- `enqueue_count - dequeue_count` 偶尔扩大，但最终能收敛

这表示：

- 发送链路已经从“无界积压”变成“受控积压”


## 11. 推荐的实施顺序

可以按下面顺序推进：

1. 保留当前 metrics 观测
2. 先改 `broadcast.rs`
3. 压测并观察指标
4. 如果 `outbound_pending_messages` 仍持续上涨，再改 per-peer bounded queue
5. 如果 retry 仍明显堆积，再限制 retry queue
6. 最后按需要补发送超时

这样做的好处是：

- 每一步都容易验证
- 每一步都容易回滚
- 不会一次改太多导致问题归因困难


## 12. 一句话结论

如果确认 `OutboundMessage` 链路是主因，而又不希望改动过大，那么最值得优先做的是：

1. 限制 `broadcast` 内部并发
2. 把 per-peer outbound queue 改成 bounded

这两步通常就足够把“内存持续上涨”压成“有限平台”，而且改动范围相对最小。
