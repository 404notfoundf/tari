根据 `test/doc/leak.md` 中的线索，对相关代码链路进行了进一步排查。结论是：

这更像是“持续内存上涨 / 在途对象堆积 / 队列背压失控”，不太像 Rust 意义上的“真实内存泄露（对象永久无法释放）”。


1. RandomXFactory

代码位置：`base_layer/core/src/proof_of_work/randomx_factory.rs`

关键代码：

- `vms: HashMap<Vec<u8>, (Instant, RandomXVMInstance)>`
- `RandomXVMInstance::create` 注释中说明 light mode 单个 VM 大约需要 `256MB`
- `create()` 中如果 key 命中则直接复用
- `self.vms.len() >= self.max_vms` 时会移除最老的 VM

分析：

- 每次 key 变化时，确实可能新建一个较大的 RandomX VM 实例
- 由于 key 与 Monero / Tari RandomX 算法相关，key 会按区块周期变化，所以会出现阶段性内存上升
- 但是这里有 `max_vms` 上限控制，不会无限插入
- 因此它更像是“受限缓存带来的大块内存占用”，而不是持续泄露

结论：

- `RandomXFactory` 会造成明显的瞬时内存抬升
- 但不是最像“持续上涨不回落”的主因


2. BroadcastTask 的无界并发发送

代码位置：`comms/dht/src/outbound/broadcast.rs:204-223`

关键代码：

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

分析：

- `messages` 是一次广播为所有目标节点生成的一批消息
- `.unordered()` 没有限制并发度
- 也就是说，单次广播会把所有目标节点的发送 future 同时驱动起来
- 如果下游发送速度慢、网络阻塞、节点较多，那么这些 future 会长时间挂住
- 如果上游持续广播，新一批 future 会继续叠加

结果：

- 在途 `DhtOutboundMessage` 数量持续增加
- 相关 reply channel、send state、日志对象等一起堆积
- RSS 看起来就像“内存泄露”一样逐步上涨

结论：

- 这是最可疑的主因之一


3. 单次广播会为每个 peer 生成一个 DhtOutboundMessage

代码位置：`comms/dht/src/outbound/broadcast.rs:398-455`

关键逻辑：

- `selected_peers.into_iter().map(|node_id| ...)`
- 为每个节点生成一个独立的 `DhtOutboundMessage`
- 同时创建 `oneshot::channel()` 和 `MessageSendState`

分析：

- 虽然 `body` 使用的是 `Bytes`，多数情况下底层数据是共享的，不一定是深拷贝
- 但消息外壳、reply 通道、状态对象仍然是按 peer 数量线性增长
- 当广播频率高、目标节点多时，即使 body 共享，控制结构也会明显放大内存占用

结论：

- 这是 broadcast 无界并发问题的放大器


4. serialize 阶段会重新编码，产生新的独立缓冲

代码位置：`comms/dht/src/outbound/serialize.rs:90-115`

关键代码：

```rust
let envelope = DhtEnvelope::new(dht_header, body.into());
let body = Bytes::from(envelope.to_encoded_bytes());
```

分析：

- `to_encoded_bytes()` 最终会走 protobuf/prost 的编码分配流程
- 也就是说，每个目标节点都会重新生成一份编码后的 `Vec<u8>`
- 即使上层 `body` 是共享的，到这里仍然要为每个目标节点各分配一份网络发送缓冲
- 如果消息体本身较大，例如区块、交易、同步数据，这里的额外内存开销会非常明显

结论：

- `prost::Message::encode_to_vec` 或 `to_encoded_bytes()` 的上涨更像是症状表现
- 根因还是前面无界并发导致大量消息同时停留在编码后阶段


5. DHT outbound 入口就是无界队列

相关位置：

- `comms/dht/src/outbound/requester.rs`
- `comms/dht/src/dht.rs`

现象：

- outbound request 使用的是 `mpsc::UnboundedSender<DhtOutboundRequest>`

分析：

- 如果上游产出消息速度大于下游处理速度，消息会先堆积在 DHT outbound 的入口队列
- 这些消息在进入 `broadcast` 之前就已经占用了内存
- 如果消息体较大，入口队列本身就会成为明显的内存增长点

结论：

- 这是一个额外的重要风险点


6. 外层 BoundedExecutor 不能解决广播内部的无界 fan-out

代码位置：

- `comms/core/src/pipeline/outbound.rs`
- `comms/core/src/bounded_executor.rs`

分析：

- `Outbound::run` 的确使用了 `BoundedExecutor`
- 它限制的是“同时运行多少个 pipeline task”
- 但单个 pipeline task 内部，`BroadcastTask::handle()` 仍然会对全部消息 `.unordered()` 并发发送
- 所以外层虽然有限流，内层 fan-out 仍然可能很大

结论：

- 不能因为存在 `BoundedExecutor` 就认为这里已经具备充分背压控制


7. messaging 层每个 peer 的发送队列也是无界的

代码位置：`comms/core/src/protocol/messaging/protocol.rs:275-335`

关键代码：

```rust
active_queues: HashMap<NodeId, mpsc::UnboundedSender<OutboundMessage>>
...
let (msg_tx, msg_rx) = mpsc::unbounded_channel();
```

分析：

- 每个 peer 都维护一个独立的无界发送队列
- 如果某个 peer 连接慢、substream 阻塞、对端读取慢，那么消息会持续积压在这个 peer 的队列中
- 上层 broadcast 即使不断投递，也不会触发硬性背压，而是继续占内存

结论：

- 这是最重要的隐藏问题之一
- 即使 broadcast 层不是特别夸张，单个慢 peer 也可能拖出长时间的内存上涨


8. retry queue 也是无界队列

代码位置：

- `comms/core/src/protocol/messaging/protocol.rs:136-151`
- `comms/core/src/protocol/messaging/outbound.rs:303-317`

分析：

- 连接断开后，尚未发出的消息会被重新转移到 retry queue
- retry queue 同样是 `mpsc::unbounded_channel()`
- 网络不稳定时，消息可能在“peer 队列 -> retry 队列 -> 再次发送”之间来回停留
- 虽然这不会制造全新消息，但会延长消息生命周期，导致内存长时间不下降

结论：

- 这是内存迟迟不回落的重要原因之一


9. dedup cache 不是主要原因，但会增加长期资源占用

代码位置：

- `comms/dht/src/config.rs`
- `comms/dht/src/dedup/dedup_cache.rs`

默认配置：

- `dedup_cache_capacity = 50_000`
- `dedup_cache_trim_interval = 12 hours`

分析：

- dedup cache 主要是 SQLite 持久化表，不是简单内存 HashMap
- 它更可能带来数据库体积增长、page cache 增长、长期资源占用
- 但它不像 outbound/broadcast 这一链路那样，直接解释“短时间持续爬升”的 RSS 问题

结论：

- 不是当前最主要的泄露嫌疑点
- 但可以作为长期占用的辅助因素考虑


10. 综合判断

如果按“导致持续内存上涨”的可疑程度排序，大致如下：

1. `broadcast.rs` 中 `.call_all(...).unordered()` 的无界 fan-out 并发
2. `protocol.rs` 中每个 peer 的 `unbounded_channel`
3. retry queue 的 `unbounded_channel`
4. DHT outbound 入口 `UnboundedSender<DhtOutboundRequest>`
5. `serialize.rs` 中每个目标节点重新编码生成独立缓冲
6. `RandomXFactory` 的大对象缓存切换
7. dedup cache / SQLite page cache


11. 最终结论

从代码上看，目前更像是“逻辑上的内存堆积”而不是“对象永久泄露”：

- 没有发现明显的 `Box::leak`、`mem::forget`、循环 `Arc` 之类典型 Rust 泄露模式
- 但存在多层无界队列和无界并发发送
- 当广播频率高、扇出大、网络发送慢、部分 peer 阻塞时，消息会在多个层级中长期堆积
- `serialize` 又会为每个目标节点重新编码分配新的发送缓冲

因此，当前最像“内存泄露”的根因是：

`outbound -> broadcast -> serialize -> messaging` 这一整条发送链路缺少足够的背压和限流，导致消息在途堆积，内存持续上涨。

12. 建议后续重点观察的指标

建议继续观察以下内容，用来进一步确认：

1. 广播频率是否过高
2. `select_peers` 每次选中的 peer 数量
3. 单条消息体大小，尤其是区块、交易、同步数据
4. 某些 peer 是否长期发送缓慢
5. per-peer outbound queue 长度
6. retry queue 长度
7. outbound request 入口队列积压量
8. serialize 后实际发送缓冲大小


一句话总结：

`RandomXFactory` 更像受限大缓存，不是持续泄露主因；真正更像“内存泄露”的核心原因，是 DHT outbound 广播链路中的无界并发、多个无界队列，以及每目标节点重新编码带来的内存堆积。

