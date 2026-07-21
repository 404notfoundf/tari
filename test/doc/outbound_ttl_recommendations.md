Outbound 消息 TTL 建议表

本文档回答两个问题：

1. 哪些 outbound 消息适合按“超过一定时间还没发出去就作废”处理
2. 如果要做最小改动，第一版 TTL 应该优先加在哪些消息上

注意：

- 这里的 TTL 不是协议规范，只是工程建议
- TTL 的目标是避免低价值旧消息长期占用内存
- TTL 不能替代 bounded queue、背压和限并发


## 1. 判断原则

一条消息适不适合 TTL，主要看这 4 点：

1. 消息是不是强时效
2. 旧消息是否会被新消息自然替代
3. 丢失一条是否会破坏协议正确性
4. 网络中是否大概率还会再次发送同类消息

如果满足：

- 强时效
- 可重复发送
- 丢失不会破坏核心语义

那么适合 TTL。

如果满足：

- 请求/响应语义明确
- 上层依赖结果
- 丢失会导致流程中断

那么不适合 TTL，应该走“超时失败”而不是“过期静默丢弃”。


## 2. 建议矩阵

### 2.1 `Ping`

建议：

- **适合 TTL**

建议窗口：

- `1 x auto_ping_interval`
- 如果没有配置 `auto_ping_interval`，可参考 `5s ~ 30s`

原因：

- `Ping` 的目标是活性探测和时延测量
- 旧 ping 送达意义很低
- 下一轮 ping 会自然替代上一轮

第一版建议：

- 如果在发送队列中等待时间超过 `auto_ping_interval`，直接丢弃


### 2.2 `Pong`

建议：

- **适合 TTL**

建议窗口：

- `1s ~ 10s`

原因：

- `Pong` 是对 `Ping` 的即时响应
- 太晚发出的 `Pong` 基本已经失去价值

第一版建议：

- 在 retry 路径中不保留 `Pong`
- 如果排队超时，直接丢


### 2.3 `Join`

建议：

- **适合 TTL**

建议窗口：

- `10s ~ 60s`

原因：

- `Join` 属于网络加入通知
- 后续通常还会重新广播
- 旧 join 长期堆积价值很低

第一版建议：

- 队列等待超过 `30s` 可直接丢弃


### 2.4 `Propagate Join`

建议：

- **适合 TTL**

建议窗口：

- `5s ~ 30s`

原因：

- 本质上是传播型 gossip
- `no_wait` 语义本来就更偏 best-effort
- 旧消息继续排队意义不大

第一版建议：

- 比 `Join` 更激进
- 等待超过 `10s ~ 15s` 可直接丢弃


### 2.5 `Discovery`

建议：

- **适合有限 TTL**

建议窗口：

- `10s ~ 60s`

原因：

- 发现消息有业务价值
- 但时效性也很强
- 过旧 discovery 即使发出去也可能已不匹配当前网络状态

第一版建议：

- 不要无限 retry
- 可给 `30s` 左右 TTL
- 过期后失败返回


### 2.6 `DiscoveryResponse`

建议：

- **不建议做静默 TTL 丢弃**

建议窗口：

- 不做“过期即丢”
- 如需限制，应做“发送超时失败”

原因：

- 这是 discovery 的直接响应
- 更接近 request/response
- 静默丢弃容易让上层流程异常但不明显

第一版建议：

- 保留
- 只做超时失败和有限重试


### 2.7 `BaseNodeRequest`

建议：

- **不建议做 TTL 丢弃**

建议窗口：

- 不做“过期即丢”
- 应做“请求超时失败”

原因：

- 属于明确请求语义
- 上层通常在等结果

第一版建议：

- bounded queue
- 超时失败
- 显式错误返回


### 2.8 `BaseNodeResponse`

建议：

- **不建议做 TTL 丢弃**

建议窗口：

- 不做“过期即丢”
- 应做“发送超时失败”

原因：

- 响应消息不应被静默吞掉

第一版建议：

- 有限重试
- 超时失败


### 2.9 `NewTransaction` flood

建议：

- **可做保守 TTL**

建议窗口：

- `10s ~ 120s`

原因：

- 有价值，但属于 flood/gossip 传播
- 对每个 peer 的每一条都必达通常不是必须
- 但 TTL 不宜像 `Ping` 那样过短

第一版建议：

- 先不要优先上 TTL
- 先做慢 peer 降级、限并发、bounded queue
- 如果仍堆积，再考虑给 flood 一个较宽松 TTL


## 3. 推荐分层

### 3.1 第一优先级：强烈建议 TTL

- `Ping`
- `Pong`
- `Join`
- `Propagate Join`

特点：

- 强时效
- 后续会重复发送
- 最适合先下手


### 3.2 第二优先级：可加有限 TTL

- `Discovery`
- `NewTransaction` flood

特点：

- 有业务价值
- 但不应无限期积压


### 3.3 不建议 TTL，改为超时失败

- `BaseNodeRequest`
- `BaseNodeResponse`
- `DiscoveryResponse`

特点：

- 不应该静默过期
- 应明确向上层暴露失败


## 4. 第一版最小落地建议

如果只想做一版最小改动，建议顺序如下：

1. 先给这些消息加 TTL：
   - `Ping`
   - `Pong`
   - `Join`
   - `Propagate Join`

2. `Discovery` 暂时只做：
   - 有限 retry
   - 发送超时
   - 可选较宽松 TTL

3. 暂时不要对下面这些做 TTL 丢弃：
   - `BaseNodeRequest`
   - `BaseNodeResponse`
   - `DiscoveryResponse`


## 5. 建议的第一版数值

如果你现在只是要快速验证思路，可以先用下面这组保守值：

- `Ping`: `auto_ping_interval`
- `Pong`: `5s`
- `Join`: `30s`
- `Propagate Join`: `10s`
- `Discovery`: `30s`
- `NewTransaction` flood: 先不启用 TTL

这组值的目标不是最优，而是：

- 先把最没有价值的旧消息尽快清掉
- 尽量不误伤关键消息


## 6. 一句话结论

当前最适合按“超过一定时间还没发出去就没意义”处理的，是：

- `Ping`
- `Pong`
- `Join`
- `Propagate Join`

其中 `Ping/Pong` 最强时效，最适合优先做 TTL。

而下面这些不应该做“过期静默丢弃”：

- `BaseNodeRequest`
- `BaseNodeResponse`
- `DiscoveryResponse`

它们更应该做的是：

- 超时失败
- 有限重试
- 显式错误返回
