Outbound 消息丢弃策略矩阵

本文档只回答一个问题：

- 当 peer 未联通、拨号失败、发送阻塞或 retry 积压时，哪些消息可以直接丢弃，哪些不能直接丢弃？

这里的结论不是协议规范，而是基于当前代码调用点与消息语义做出的工程化分层，目标是给后续最小改动提供直接依据。


## 1. 判断原则

判断是否可以丢弃，先看 4 件事：

1. 这是不是 request/response 语义
2. 上层是否显式等待结果
3. 这条消息是否强时效
4. 网络中后续是否大概率还会再次传播同类消息

如果满足：

- 强时效
- 会重复发送
- 丢一条不会破坏协议正确性

那么更适合直接丢弃或短暂失败化处理。

如果满足：

- 请求/响应语义明确
- 上层依赖结果
- 丢失会导致流程不完整

那么不适合直接丢弃，应走“有限重试 + 超时失败”。


## 2. 当前识别到的主要消息类型

### 2.1 `Ping`

代码位置：

- `base_layer/p2p/src/services/liveness/service.rs:266`
- `base_layer/p2p/src/services/liveness/service.rs:393`

发送方式：

- `send_direct_node_id`

结论：

- **可以直接丢弃**

原因：

- 活性探测消息
- 强时效
- 后续还会继续发
- 旧 ping 送达意义很低

建议策略：

- 未联通时直接失败返回
- 不进入长期 retry
- 可选：记录 drop metrics


### 2.2 `Pong`

代码位置：

- `base_layer/p2p/src/services/liveness/service.rs:279`

发送方式：

- `send_direct_unencrypted`

结论：

- **可以直接丢弃**

原因：

- 对 ping 的时效性响应
- 太晚送达通常已没有价值
- 没必要为了保住每个 pong 而长期占用内存

建议策略：

- 未联通时直接丢
- 不进入长期 retry


### 2.3 `Join`

代码位置：

- `comms/dht/src/actor.rs:543`

发送方式：

- `send_message_no_header(...broadcast(...))`

结论：

- **可以直接丢弃**

原因：

- 网络加入通知
- 广播 / gossip 语义
- 后续通常还会再次发送
- 延迟太久的旧 join 价值明显下降

建议策略：

- 队列满时丢弃
- 未联通时不进入长期 retry
- 可加 TTL


### 2.4 `Propagate Join`

代码位置：

- `comms/dht/src/inbound/dht_handler/task.rs:261`

发送方式：

- `send_raw_no_wait`

结论：

- **可以直接丢弃**

原因：

- 本身就是传播型消息
- `no_wait` 已经说明它更偏 best-effort
- 旧的 propagated join 继续排队意义不大

建议策略：

- 队列满时直接丢
- 不进 retry queue
- 可加 TTL


### 2.5 `Discovery`

代码位置：

- `comms/dht/src/discovery/service.rs:360`

发送方式：

- `send_message_no_header(...broadcast(...))`

结论：

- **条件丢弃**

原因：

- 有业务价值，但同时带明显时效性
- 过旧的 discovery 请求价值会快速下降
- 但在某些场景中，直接完全不发也会影响发现流程

建议策略：

- 允许短暂重试
- 设置 TTL
- 过期后丢弃
- 不建议无限重试或无限缓存


### 2.6 `DiscoveryResponse`

代码位置：

- `comms/dht/src/inbound/dht_handler/task.rs:446`

发送方式：

- `send_message_no_header_no_wait`

结论：

- **不建议直接丢弃**

原因：

- 这是对 discovery 的直接响应
- 更接近 request/response 语义
- 丢失会让对端 discovery 结果不完整

注意：

- 虽然调用是 `no_wait`
- 但从业务意义上看，它仍然比普通 gossip 更重要
- 这里的 `no_wait` 不能直接等价于“可随便丢”

建议策略：

- 允许有限重试
- 允许超时失败
- 不建议走长期无界 retry


### 2.7 `BaseNodeRequest`

代码位置：

- `base_layer/core/src/base_node/service/service.rs:578`

发送方式：

- `send_message`

结论：

- **不能直接丢弃**

原因：

- 更接近 RPC 请求
- 上层通常在等待响应
- 丢了会导致调用方超时或业务流程失败

建议策略：

- bounded queue
- 背压
- 发送超时
- 明确失败返回


### 2.8 `BaseNodeResponse`

代码位置：

- `base_layer/core/src/base_node/service/service.rs:477`

发送方式：

- `send_direct_unencrypted`

结论：

- **不能直接丢弃**

原因：

- 是对请求的响应
- 直接丢失会让对端请求结果缺失
- 更适合失败化，而不是静默丢弃

建议策略：

- 有限重试
- 超时失败
- 不进入无界积压


### 2.9 `NewTransaction` flood

代码位置：

- `base_layer/core/src/mempool/service/service.rs:204`

发送方式：

- `flood(...)`

结论：

- **条件丢弃**

原因：

- 交易广播有价值
- 但 flood/gossip 通常不是“对每个 peer 每条都必达”
- 网络中后续可能还会从其他 peer 再传播

建议策略：

- 对慢 peer 允许丢弃
- 队列满时允许失败
- 不建议长期无界 retry
- 一般不建议像 `Ping` 一样无条件直接丢


## 3. 汇总矩阵

### 3.1 可以直接丢弃

- `Ping`
- `Pong`
- `Join`
- `Propagate Join`

适用条件：

- peer 未联通
- 队列已满
- retry 已拥塞
- 消息已明显过期


### 3.2 条件丢弃

- `Discovery`
- `NewTransaction` flood

适用策略：

- 短暂重试
- TTL
- 队列满时失败
- 对慢 peer 降级


### 3.3 不可直接丢弃

- `BaseNodeRequest`
- `BaseNodeResponse`
- `DiscoveryResponse`

适用策略：

- bounded queue
- 背压
- 超时
- 有限重试
- 显式失败返回


## 4. 最小落地建议

如果你要做最小改动，不建议一上来对所有消息统一加 TTL 或统一丢弃。

更稳的顺序是：

1. 先把下面这些从 retry 路径里排除掉：
   - `Ping`
   - `Pong`
   - `Join`
   - `Propagate Join`

2. 对下面这些保留“有限重试 + 超时失败”：
   - `Discovery`
   - `NewTransaction` flood

3. 对下面这些维持保守语义：
   - `BaseNodeRequest`
   - `BaseNodeResponse`
   - `DiscoveryResponse`

这样做的好处是：

- 改动小
- 风险可控
- 能先显著减少低价值消息积压
- 不容易误伤 request/response 链路


## 5. 一句话结论

当前最适合在“未联通 / 阻塞 / retry 积压”时直接丢弃的，是：

- `Ping`
- `Pong`
- `Join`
- `Propagate Join`

最不适合直接丢弃的，是：

- `BaseNodeRequest`
- `BaseNodeResponse`
- `DiscoveryResponse`

而 `Discovery` 和 `NewTransaction` flood 更适合走中间策略：

- 不无限保留
- 也不简单一刀切直接丢