Outbound 消息来源与价值分层草案

本文档用于回答：

1. 当前 outbound 发送链路里，主要有哪些消息来源
2. 这些消息大致在什么场景下发送
3. 哪些消息更可能是“必须保留”的
4. 哪些消息更可能是“只保留最新即可”或“best effort”
5. 哪些消息可以优先作为“旧消息清理 / TTL / 降级丢弃”的候选

注意：

- 本文档是基于当前代码调用点的第一版分类草案
- 重点是帮助判断“哪些消息值得保，哪些消息可以降级”
- 不是严格协议规范


## 1. 发送链路入口

核心入口位于：

- `comms/dht/src/outbound/requester.rs`

这里的主要发送方式包括：

- `send_direct_node_id`
- `send_direct_unencrypted`
- `send_message`
- `send_message_no_header`
- `send_message_no_header_no_wait`
- `send_raw_no_wait`
- `broadcast`
- `flood`

这意味着从价值上，当前 outbound 消息至少可以先按以下维度判断：

- 单播还是广播
- 是否等待结果
- 是否 no_wait
- 是否会 fan-out 到多个 peer

通常来说：

- 广播 / flood / propagate 更适合做降级处理
- direct request/response 更偏向要保留


## 2. 当前代码里已识别出的主要消息来源

### 2.1 Liveness Ping / Pong

代码位置：

- `base_layer/p2p/src/services/liveness/service.rs`

发送点：

- `send_direct_node_id` 发送 Ping
- `send_direct_unencrypted` 发送 Pong

关键代码位置：

- `send_ping`：`service.rs:255`
- `send_pong`：`service.rs:276`
- `start_ping_round`：`service.rs:385`

消息特点：

- 高频
- 小消息
- 单播
- 用于活性检查

价值判断：

- `Ping`
  - 有价值，但通常不是“必须每一条都绝对送达”
  - 更偏向时效性消息
  - 旧 ping 价值下降很快

- `Pong`
  - 是对 ping 的响应
  - 也偏时效性
  - 太晚到达的价值明显下降

建议分类：

- `Ping/Pong` 更接近：
  - `latest_or_ttl`
  - 而不是 `must_deliver_every_message`

结论：

- 这是“可考虑 TTL / 过期清理 / 慢 peer 降级”的候选


## 3. Mempool NewTransaction Flood

代码位置: 

- `base_layer/core/src/mempool/service/service.rs`

发送点：

- `flood(...)`

关键代码位置：

- `handle_outbound_tx`：`service.rs:197`

消息特点：

- 广播 / flood
- fan-out 明显
- 交易消息
- 频率可能高

价值判断：

- 交易广播是有价值的
- 但在 gossip/flood 场景里，通常不要求“对每个 peer 的每一条 flood 都可靠必达”
- 网络里往往还会从别的 peer 再次收到同类传播

建议分类：

- 更接近：
  - `important_but_best_effort_broadcast`

结论：

- 不适合简单按“旧消息全清”
- 但适合：
  - 限 fan-out 并发
  - 限队列
  - 对慢 peer 降级
- 一般不建议把它当成严格逐 peer 必达消息


## 4. Base Node Request / Response

代码位置：

- `base_layer/core/src/base_node/service/service.rs`

发送点：

- `send_direct_unencrypted` 发送 `BaseNodeResponse`
- `send_message` 发送 `BaseNodeRequest`

关键代码位置：

- 响应：`service.rs:476`
- 请求：`service.rs:577`

消息特点：

- 更像 request/response RPC 语义
- 通常与某个请求 key、调用方 reply 绑定
- 直接影响上层请求流程

价值判断：

- 这类消息价值较高
- 通常不适合做“旧消息淘汰”
- 尤其不适合简单按 TTL 丢弃

建议分类：

- `must_deliver_or_explicit_fail`

结论：

- 这类消息不应该优先作为“清理旧消息”的候选
- 更适合：
  - bounded queue
  - 背压
  - 明确失败返回


## 5. Discovery Broadcast

代码位置：

- `comms/dht/src/discovery/service.rs`

发送点：

- `send_message_no_header(... .broadcast(...))`

关键代码位置：

- `service.rs:359`

消息特点：

- 广播
- fan-out
- discovery 类型消息
- 主要用于发现目标 peer

价值判断：

- discovery 有业务价值
- 但它本身带有明显的“时效性”
- 过旧的 discovery 消息价值通常会下降

建议分类：

- `ttl_friendly_broadcast`

结论：

- 这是比较适合做：
  - TTL 清理
  - fan-out 限流
  - 慢 peer 降级


## 6. Join Message Propagation / DiscoveryResponse

代码位置：

- `comms/dht/src/inbound/dht_handler/task.rs`
- `comms/dht/src/actor.rs`

发送点：

- `send_raw_no_wait` 传播 Join
- `send_message_no_header_no_wait` 发送 DiscoveryResponse
- `send_message_no_header(...broadcast...)` 广播 Join

关键代码位置：

- 传播 Join：`task.rs:260`
- DiscoveryResponse：`task.rs:445`
- 广播 Join：`actor.rs:542`

消息特点：

- Join 广播 / propagate：
  - 明显是 gossip / 网络加入通知
  - 往往会重复出现
  - 旧 join 的价值通常不高

- DiscoveryResponse：
  - 是某次发现请求的直接响应
  - 价值高于普通 gossip

价值判断：

- `Join`
  - 更接近：
    - `best_effort_or_ttl`
  - 很适合做降级和过期清理候选

- `DiscoveryResponse`
  - 更接近：
    - `must_deliver_or_explicit_fail`
  - 不建议随便清理


## 7. 初步价值分层

基于当前已识别的发送点，可以先做如下草案。

### 7.1 更像必须保留 / 明确失败的消息

- `BaseNodeRequest`
- `BaseNodeResponse`
- `DiscoveryResponse`

处理建议：

- 不要简单做“旧消息清理”
- 更适合：
  - bounded queue
  - 背压
  - 超时
  - retry 上限
  - 明确失败路径


### 7.2 更像有价值但属于 best effort 的广播消息

- `NewTransaction` flood

处理建议：

- 不一定要保证对每个 peer 必达
- 更适合：
  - 限 fan-out 并发
  - 慢 peer 降级
  - bounded queue
- 不建议最先做“盲目 TTL 丢弃”，但可以考虑对慢 peer 失败化


### 7.3 更像时效性强、旧消息价值下降很快的消息

- `Ping`
- `Pong`
- `Discovery`
- `Join`
- `Propagated Join`

处理建议：

- 这是最适合优先尝试：
  - TTL 清理
  - 覆盖旧消息
  - 队列满时丢弃
  - no_wait / best effort 降级


## 8. 最适合优先排查“是否没必要积压”的消息

如果目标是找“哪些消息可能没必要一直堆着”，建议优先从这些开始：

1. `Join`
2. `Discovery`
3. `Ping/Pong`
4. `Propagate Join`

原因：

- 这些消息都更偏网络维护/发现/活性
- 明显具有时效性
- 旧消息价值衰减快
- 比较适合做 TTL 或 best-effort 降级


## 9. 不建议优先清理的消息

不建议第一时间做“旧消息淘汰”的：

1. `BaseNodeRequest`
2. `BaseNodeResponse`
3. `DiscoveryResponse`

原因：

- 它们更像 request/response 语义
- 上层更可能依赖结果
- 过早清理更容易引入协议层错误


## 10. 如果要做“清理旧消息”，建议的切入顺序

推荐顺序：

1. 先对 `Join` / `Discovery` / `Ping/Pong` 做价值确认
2. 再判断这些消息是否可以：
   - 设置 TTL
   - 队列满时丢弃
   - 覆盖旧消息
3. 保留 `BaseNodeRequest/Response` 这类消息走更保守路径

这样做的原因是：

- 先从“明显时效性强”的消息下手，风险最小
- 避免误伤 request/response 语义消息


## 11. 一句话结论

如果你的目标是找“哪些 outbound 消息其实没有必要长期堆积”，那么第一批最值得怀疑和最适合做降级/过期清理候选的是：

- `Join`
- `Discovery`
- `Ping/Pong`

而更不应该优先动的则是：

- `BaseNodeRequest`
- `BaseNodeResponse`
- `DiscoveryResponse`

也就是说，应该先按消息价值分层，而不是对所有 `OutboundMessage` 一刀切做统一清理。
