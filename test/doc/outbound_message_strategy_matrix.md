Outbound 消息策略分层矩阵

本文档是在 `outbound_message_classification.md` 的基础上进一步收敛，目标是把当前 outbound 消息按处理策略分成 4 类：

1. `must_deliver`
2. `latest_only`
3. `best_effort`
4. `ttl_candidate`

这样后续如果要做：

- bounded queue
- 过期清理
- 丢弃旧消息
- 慢 peer 降级

就可以直接按分类落策略，而不是对所有 `OutboundMessage` 一刀切。


## 1. 分类原则

### 1.1 `must_deliver`

定义：

- 业务上不能随意丢
- 更接近 request/response
- 上层依赖明确结果
- 不适合“旧消息直接清掉”

更适合的策略：

- bounded queue
- 背压
- 超时
- retry 上限
- 明确失败返回


### 1.2 `latest_only`

定义：

- 只要最新状态即可
- 旧消息价值会被新消息覆盖
- 不需要严格逐条送达

更适合的策略：

- 覆盖旧消息
- 同类消息只保留最新一条
- bounded latest-value queue


### 1.3 `best_effort`

定义：

- 消息有价值
- 但不值得为了每条都保留而让系统无限涨内存
- 通常属于广播 / gossip / 冗余传播

更适合的策略：

- 限并发
- bounded queue
- 队列满时失败或丢弃
- 慢 peer 降级


### 1.4 `ttl_candidate`

定义：

- 明显具有时效性
- 延迟太久后价值显著下降
- 旧消息继续保留意义不大

更适合的策略：

- 设置 TTL
- 过期即丢
- 与 `best_effort` 结合使用


## 2. 第一版策略矩阵

### 2.1 `must_deliver`

建议归入：

- `BaseNodeRequest`
- `BaseNodeResponse`
- `DiscoveryResponse`

主要代码位置：

- `base_layer/core/src/base_node/service/service.rs`
- `comms/dht/src/inbound/dht_handler/task.rs`

原因：

- 这几类更像请求与响应语义
- 上层通常在等待结果
- 直接清理旧消息容易造成行为错误

建议策略：

- 不做 TTL 清理
- 不做简单“只保留最新”
- 首选：
  - bounded queue
  - 背压
  - 超时
  - 显式失败


### 2.2 `latest_only`

当前代码里，没有特别明显已经可以直接归为 `latest_only` 的消息类型。

原因：

- 当前已识别的几类消息大多是 request/response、gossip 或时效性广播
- 暂时没有看到非常明确的“同类状态更新只保留最新值即可”的发送点

但如果后续确认某些消息属于：

- 最新状态广播
- 最新进度通知
- 最新快照同步

那么这类消息应优先归入 `latest_only`。

当前建议：

- 暂时保留为空分类
- 后续重点排查是否存在“状态类 outbound 消息”


### 2.3 `best_effort`

建议归入：

- `NewTransaction` flood
- `Join`
- `Propagate Join`
- `Discovery`
- `Ping`
- `Pong`

主要代码位置：

- `base_layer/core/src/mempool/service/service.rs`
- `comms/dht/src/actor.rs`
- `comms/dht/src/inbound/dht_handler/task.rs`
- `comms/dht/src/discovery/service.rs`
- `base_layer/p2p/src/services/liveness/service.rs`

原因：

- 这些消息都不是严格意义上的“必须逐条送达给每个 peer”
- 很多属于广播、gossip、网络维护、活性探测
- 即使丢一部分，系统通常也不至于立刻协议错误

建议策略：

- 先限制 fan-out 并发
- per-peer queue bounded
- 队列满时允许失败或丢弃
- 对慢 peer 降级


### 2.4 `ttl_candidate`

建议归入：

- `Join`
- `Propagate Join`
- `Discovery`
- `Ping`
- `Pong`

原因：

- 都明显具有时效性
- 太晚送达后价值下降很快
- 长时间堆积通常不值得

建议策略：

- 可以尝试加 TTL
- 队列中发现已过期则直接丢弃
- 与 `best_effort` 组合使用


## 3. 各类消息的推荐操作

### 3.1 `must_deliver`

推荐：

- 保留
- 队列加上限
- 满了以后阻塞上游或明确失败

不推荐：

- 统一 TTL 清理
- 统一丢弃旧消息
- 统一覆盖旧消息


### 3.2 `latest_only`

推荐：

- 用覆盖模型代替普通 FIFO 队列
- 同类型只保留最新一条

不推荐：

- 长队列排队等待


### 3.3 `best_effort`

推荐：

- 限流
- bounded queue
- 队列满时直接失败或丢弃

不推荐：

- 为了保持每条消息都不丢而无限积压


### 3.4 `ttl_candidate`

推荐：

- 明确 TTL
- 过期即丢
- 结合 metrics 统计丢弃量

不推荐：

- 无限制等待发送


## 4. 当前最值得优先改造的候选

如果目标是“先做小改动、先降低内存上涨风险”，建议优先盯住这些消息类型：

1. `Join`
2. `Propagate Join`
3. `Discovery`
4. `Ping/Pong`

原因：

- 这些消息同时满足：
  - 有一定发送频率
  - 具有时效性
  - 更偏网络维护/活性/发现
  - 更适合 best-effort 或 TTL

也就是说，这些消息最适合优先作为：

- 过期清理候选
- 慢 peer 降级候选
- 队列满时丢弃候选


## 5. 当前不建议优先动的候选

如果目标是低风险，不建议最先动这些：

1. `BaseNodeRequest`
2. `BaseNodeResponse`
3. `DiscoveryResponse`

原因：

- 这些消息更接近“调用链的一部分”
- 更可能被上层显式依赖
- 改成 TTL/清理很容易引入隐藏错误


## 6. 第一版落地建议

如果后续真的要开始做代码层策略改造，建议按这个顺序：

1. 对全局链路先做：
   - `broadcast` 限并发
   - per-peer queue bounded

2. 对 `best_effort + ttl_candidate` 类消息：
   - 评估是否加 TTL
   - 队列满时允许失败或丢弃

3. 对 `must_deliver` 类消息：
   - 保持保守策略
   - 做 bounded + 背压 + 超时

这样可以做到：

- 不把所有消息一刀切
- 优先从时效性消息下手
- 降低误伤关键请求/响应链路的风险


## 7. 简化版结论表

### 7.1 `must_deliver`

- `BaseNodeRequest`
- `BaseNodeResponse`
- `DiscoveryResponse`

### 7.2 `latest_only`

- 当前尚未明确识别出典型候选

### 7.3 `best_effort`

- `NewTransaction` flood
- `Join`
- `Propagate Join`
- `Discovery`
- `Ping`
- `Pong`

### 7.4 `ttl_candidate`

- `Join`
- `Propagate Join`
- `Discovery`
- `Ping`
- `Pong`


## 8. 一句话结论

当前最合理的方向不是对所有 outbound 消息统一做“旧消息清理”，而是：

- `must_deliver` 走保守路径
- `best_effort` 走限流和失败化路径
- `ttl_candidate` 优先考虑过期清理

其中最适合先试 TTL / 丢弃 / 降级的，是：

- `Join`
- `Discovery`
- `Ping/Pong`
