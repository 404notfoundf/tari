Discovery TTL 最小实现清单

本文档说明 `Discovery` 消息是否适合做 TTL，以及如何以最小改动方式实现。

目标是：

- `Discovery` 可以短暂 retry
- 但不能无限积压
- 超过 TTL 后直接失败释放


## 1. 结论

`Discovery` **适合做 TTL**，但应比 `Ping/Pong`、`Propagate Join` 更保守。

建议第一版：

- 允许 retry
- 允许短暂排队
- 超过 TTL 后直接丢弃并失败返回

建议 TTL：

- 保守值：`30s`
- 更保守：`60s`


## 2. 为什么 `Discovery` 适合 TTL

`Discovery` 的作用不是普通业务消息传输，而是：

- 当本地还不知道目标 peer 的可用路径时
- 向网络发起“查找这个 peer”的控制消息
- 目标 peer 解密后回 `DiscoveryResponse`

相关位置：

- `comms/dht/src/discovery/service.rs:336`
- `comms/dht/src/proto/dht.proto:24`

它的特点是：

- 有业务价值
- 但强时效
- 调用方通常是在“当前时刻”想找到目标 peer

如果一条 `Discovery` 在几十秒后才真正发出去：

- 原调用方可能已经超时
- 拓扑状态可能已经变了
- 这次 discover 的原始目的可能已经不存在

所以它不适合无限期保留。


## 3. 为什么不能像 `Ping/Pong` 一样激进

`Ping/Pong`：

- 主要是活性和延迟探测
- 过期后几乎立刻没价值

`Discovery`：

- 是控制类消息
- 价值高于纯活性探测
- 丢得太早可能影响找 peer 成功率

因此更合理的策略不是：

- “发不出去就立刻丢”

而是：

- “允许短暂 retry，但有 TTL 上限”


## 4. 当前发送点

文件：

- `comms/dht/src/discovery/service.rs`

关键发送点：

- `comms/dht/src/discovery/service.rs:359`

当前逻辑：

- `send_message_no_header(...)`
- `broadcast(Vec::new())`
- `with_destination(destination)`
- `with_encryption(OutboundEncryption::EncryptFor(dest_public_key))`
- `with_dht_message_type(DhtMessageType::Discovery)`

当前没有显式 TTL。


## 5. 推荐实现方式

沿用与 `Ping/Pong`、`Join` 相同的机制：

1. 发送时写入 `expires`
2. outbound 发送前检查是否过期
3. disconnect 后转 retry 前再次检查是否过期
4. 过期则直接失败并释放

这样不需要为 `Discovery` 再单独发明一套清理逻辑。


## 6. 前置依赖

这份方案默认已经有以下基础能力：

- `comms/dht/src/outbound/message_params.rs`
  - `FinalSendMessageParams.expires`
  - `SendMessageParams::with_expires(...)`

- `comms/core/src/protocol/messaging/outbound.rs`
  - `is_expired_message(&OutboundMessage)`
  - 发送前过期检查
  - retry 前过期检查

如果前面两类消息已经按这个方案改了，`Discovery` 只需要在发送点补 TTL 即可。


## 7. 代码修改建议

文件：

- `comms/dht/src/discovery/service.rs`

位置：

- `comms/dht/src/discovery/service.rs:359`

当前发送代码大致是：

```rust
self.outbound_requester
    .send_message_no_header(
        SendMessageParams::new()
            .broadcast(Vec::new())
            .with_destination(destination)
            .with_debug_info(...)
            .with_encryption(...)
            .with_dht_message_type(DhtMessageType::Discovery)
            .finish(),
        discover_msg,
    )
```

建议补上：

```rust
.with_expires(EpochTime::now() + Duration::from_secs(30))
```

第一版建议先写固定值 `30s`。

如果后续需要更灵活，可以把它提成配置项。


## 8. `DiscoveryResponse` 不要一起套用

这里要特别区分：

- `Discovery`
  - 可以 TTL 失败释放

- `DiscoveryResponse`
  - 不建议做静默 TTL 丢弃

原因：

- `Discovery` 是发起动作
- `DiscoveryResponse` 是对发起动作的响应
- 响应消息更接近 request/response 语义

所以：

- `Discovery` 适合 TTL
- `DiscoveryResponse` 更适合超时失败，而不是“过期后静默清掉”


## 9. 建议 TTL 数值

第一版建议：

- `Discovery`: `30s`

更保守版本：

- `Discovery`: `60s`

不建议：

- 小于 `10s`
  - 过于激进，可能影响正常发现成功率

- 无限期保留
  - 容易造成慢 peer / 断连场景下的积压


## 10. 预期行为变化

### 10.1 正向收益

- 减少陈旧 discover 请求长期占用 retry/queue
- 减少无效控制消息在网络恢复后集中发送
- 降低 outbound 内存持续增长风险


### 10.2 可能副作用

- 极端慢网络下，部分 discover 请求可能在真正发出前过期
- 某些发现动作会更早失败，需要上层重新发起

这个代价通常是可接受的，因为：

- `Discovery` 本身就是一次“当前时间点”的查找请求
- 如果太晚才发出去，成功价值也已经下降


## 11. 测试建议

### 11.1 过期 `Discovery` 不会真正发送

思路：

- 构造带过期时间的 `Discovery`
- 让其在 queue 中等待到过期
- 断言：
  - 不会真正写到 sink
  - 会失败返回


### 11.2 过期 `Discovery` 不进入 retry

思路：

- 构造即将过期的 `Discovery`
- 制造 disconnect
- 断言：
  - 不会进入 retry queue
  - `retry_queue_messages` 不增加


## 12. 落地顺序建议

建议顺序：

1. `Ping/Pong TTL`
2. `Propagate Join TTL`
3. `Join TTL`
4. `Discovery TTL`

原因：

- `Discovery` 的业务价值比前面三类更高
- 放在第四步更稳，便于先观察前几类的收益和副作用


## 13. 一句话结论

`Discovery` 可以按 TTL 释放，但应比 `Ping/Pong` 更保守。

最小实现方式仍然是：

- 发送时写 `expires`
- outbound 发送前 / retry 前统一做过期检查
- 过期则失败释放

第一版建议值：

- `Discovery`: `30s`
