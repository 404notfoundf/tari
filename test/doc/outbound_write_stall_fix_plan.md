# Outbound 写入阻塞导致内存持续上涨的改动方案

本文档基于 `test/doc/leak.md` 中的 pprof 图片和当前 outbound 代码整理。

目标是在不大改整体 DHT、连接管理和消息接口的前提下，解决以下问题：

```text
某个 peer 长期不读取数据
    ↓
Forward::new(...) 长期 Pending
    ↓
该 peer 后续消息无法 dequeue
    ↓
后续消息即使已经过期，也没有机会执行 is_expired()
    ↓
新区块继续进入无界队列
    ↓
编码后的 Bytes、future、oneshot 等对象持续占用内存
```


## 1. pprof 证据与问题定位

`test/doc/leak.md` 的 Outbound 前后图片显示：

- `BroadcastTask<S>::handle` 从约 `326MB` 增长到约 `2.01GB`
- `tower::util::oneshot::Oneshot::poll` 增长到约 `2.01GB`
- `futures_util::stream::for_each::ForEach::poll` 增长到约 `1.94GB`

对应代码：

- `comms/dht/src/outbound/broadcast.rs`

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

真正的网络写入位于：

- `comms/core/src/protocol/messaging/outbound.rs`

```rust
super::forward::Forward::new(stream, sink.sink_map_err(Into::into)).await?;
```

`Forward` 内部可能长期阻塞在：

```text
sink.poll_ready()
sink.poll_flush()
yamux 流控
TCP send buffer
对端读取速度
```

当前 `Forward` 只缓存一条正在写入的消息，但前面的消息写不出去时，该 peer 对应无界队列中的所有后续消息都会继续保留。


## 2. 为什么当前过期清理不能彻底解决

需要先区分原始代码和当前工作区中的实验性修改。

### 2.1 原始代码没有 outbound 过期清理

原始代码中不存在以下发送前检查：

```rust
if out_msg.is_expired() {
    out_msg.reply_fail(SendFailReason::Dropped);
    return future::ready(None);
}
```

原始 `OutboundMessage` 也没有可供 messaging 层判断的 `expires_at` 字段。DHT 消息的 `expires` 只写入发送消息的 DHT header，主要由接收方和转发方判断是否丢弃。

因此，原始代码中的行为是：

```text
消息进入本机 per-peer outbound queue
    ↓
即使 DHT header 中的 expires 已经过期
    ↓
本机 messaging outbound 仍然不知道它已经过期
    ↓
消息仍然可能被发送或进入 retry queue
    ↓
接收方收到后才可能根据 DHT header 丢弃
```

这意味着原始代码中的消息 TTL 不能主动释放本机 outbound 队列中的消息。

当前工作区增加的 `expires_at` 传递、发送前检查和 retry 前检查，只能作为第一层止血措施，并不是原始代码已有的行为。

### 2.2 当前实验性过期清理仍有两个限制

当前增加的发送前检查可以清理正常 dequeue 的过期消息，但它仍存在两个限制。

#### 2.2.1 后续消息无法被检查

如果消息 A 已经进入 `Forward` 并阻塞：

```text
A 尚未过期，通过 is_expired()
    ↓
A 进入 Forward 后写入阻塞
    ↓
B、C、D 留在 messages_rx
    ↓
B、C、D 到期后仍无法执行 is_expired()
```

#### 2.2.2 已经进入 Forward 的消息失去过期信息

当前代码只向 `Forward` 传递：

```rust
out_msg.body
```

进入 `Forward` 后不再携带：

- `expires_at`
- `tag`
- `peer_node_id`
- `reply`

因此无法在写入阻塞期间再次判断当前消息是否过期。


## 3. 修改目标

本次修改需要实现：

1. 单个 peer 的网络写入不能永久阻塞。
2. 消息真正写入成功后，才能调用 `reply_success()`。
3. 写入长期没有进展时，结束当前 outbound session。
4. session 结束后，继续复用当前过期清理逻辑。
5. 不立即把所有无界队列改成有界队列。
6. 不修改 NewBlock、Ping/Pong 等上层业务接口。


## 4. 推荐方案：增加按 peer 的写入停滞超时

### 4.1 核心语义

增加一个独立的 `outbound_write_stall_timeout`：

```text
消息 TTL：
    判断消息是否还有业务价值。

写入停滞超时：
    判断当前 peer 的网络发送链路是否长时间没有任何进展。
```

这两个时间不能混用

建议初始值：

```text
outbound_write_stall_timeout = 60 秒
```

不要使用默认约 3 小时的 DHT 消息有效期作为网络写入超时。


### 4.2 改动位置

主要修改：

- `comms/core/src/protocol/messaging/outbound.rs`
- `comms/core/src/protocol/messaging/forward.rs`
- messaging 配置定义和初始化位置
- `comms/core/src/protocol/messaging/metrics.rs`

测试修改：

- `comms/core/src/protocol/messaging/test.rs`


### 4.3 推荐实现方式

不要简单对整个 `Forward::new(...)` 使用：

```rust
tokio::time::timeout(timeout, Forward::new(...))
```

因为 outbound handler 本身是一个长期运行任务。即使连接健康，只要运行时间超过 timeout，也会被错误终止。

正确方式是增加“无写入进展超时”：

```text
每次成功 start_send 或 flush 后重置计时器
    ↓
如果 poll_ready / poll_flush 连续超过指定时间仍然 Pending
    ↓
返回 WriteStalled 错误
    ↓
结束当前 outbound session
```

可以通过以下两种方式实现。

#### 方案 A：扩展当前 `Forward`

在 `Forward` 中保存：

```rust
stall_timeout
stall_sleep
```

每次发送或 flush 取得进展后重置 `stall_sleep`。如果 sink 长期 Pending 且计时器完成，则返回超时错误。

优点：

- 保留当前批量 poll 和 flush 行为。
- 对吞吐量影响较小。
- 修改集中在 `Forward`。

缺点：

- 需要让 `Forward` 能返回明确的超时错误。
- 泛型错误类型处理会增加少量复杂度。

#### 方案 B：在 outbound handler 中显式逐条发送

用循环替代 `Forward::new(...)`：

```rust
while let Some(mut out_msg) = messages_rx.recv().await {
    if out_msg.is_expired() {
        out_msg.reply_fail(SendFailReason::Dropped);
        continue;
    }

    match tokio::time::timeout(write_timeout, sink.send(out_msg.body.clone())).await {
        Ok(Ok(())) => out_msg.reply_success(),
        Ok(Err(err)) => {
            out_msg.reply_fail(...);
            return Err(err.into());
        },
        Err(_) => {
            out_msg.reply_fail(...);
            return Err(...);
        },
    }
}
```

优点：

- 行为直观。
- 消息发送成功和失败语义清晰。
- 容易记录具体 peer、tag、消息大小和耗时。

缺点：

- `SinkExt::send()` 通常会对每条消息执行 flush，可能降低吞吐量。
- 需要重新整合当前 `take_until(on_disconnect)` 行为。
- 改动比扩展 `Forward` 更大。

综合考虑，推荐优先使用 **方案 A：扩展当前 `Forward`**。


## 5. 修正 reply_success 调用时机

当前代码在消息交给 `Forward` 前就调用：

```rust
out_msg.reply_success();
```

这只能说明消息从 per-peer queue 中取出，不能说明消息已经成功写入网络。

建议修改为：

```text
消息写入 sink 成功
    ↓
再调用 reply_success()
```

如果继续使用当前 `Forward`，则需要让 `Forward` 持有完整的待发送消息，或提供发送完成回调。

这是正确性改进，但会扩大第一阶段改动范围。因此可以拆分：

1. 第一阶段先实现写入停滞超时，防止无限 Pending。
2. 第二阶段再修正 `reply_success()` 的准确语义。


## 6. 写入超时后的处理策略

发生写入停滞超时后，不建议继续使用当前 session。

推荐行为：

```text
记录 peer 写入停滞指标和日志
    ↓
结束当前 outbound handler
    ↓
关闭当前 messages_rx
    ↓
遍历队列中的剩余消息
    ↓
过期消息直接 Dropped
    ↓
未过期消息进入 retry queue
    ↓
由现有逻辑重连或失败
```

当前可复用的队列清理位置：

- `comms/core/src/protocol/messaging/outbound.rs`

```rust
while let Some(mut msg) = messages_rx.recv().await {
    if msg.is_expired() {
        msg.reply_fail(SendFailReason::Dropped);
        continue;
    }

    self.retry_queue_tx.send(msg)?;
}
```

需要注意：

- 写入超时后必须真正结束当前 session。
- 如果只是记录超时但继续等待同一个 sink，内存问题不会改善。
- retry queue 当前仍然是无界队列，因此过期检查必须保留。


## 7. 消息类型影响

### 7.1 NewBlock

新区块消息是本次重点。

如果一个 peer 长期无法接收新区块：

- 继续保留大量旧 NewBlock 意义有限。
- peer 恢复后可以通过区块同步获取缺失区块。
- 写入停滞后断开并重连，比无限保留消息更合理。

第一阶段不需要修改 NewBlock 专属逻辑，只需要确保：

- 写入不能永久阻塞。
- 过期 NewBlock 不再进入 retry。


### 7.2 Ping/Pong

过期 Ping/Pong 被丢弃是合理的，因为对应 liveness 检测窗口已经结束。

影响：

- 删除过期 Ping 后，本地可能因为没有收到 Pong 而增加该 peer 的 `failed_pings`。
- 删除过期 Pong 后，对端可能将本节点记为一次 Ping 失败。

但如果发送链路已经阻塞超过消息有效期，这种 liveness 失败结果通常符合实际连接状态。

`reply_fail(SendFailReason::Dropped)` 本身不会直接修改 `failed_pings`。


## 8. 必须增加的观测指标

为了验证修改效果，建议增加以下 Prometheus 指标：

```text
comms_messaging_outbound_write_stall_total
comms_messaging_outbound_write_stall_seconds
comms_messaging_outbound_expired_before_send_total
comms_messaging_outbound_expired_before_retry_total
comms_messaging_outbound_retry_total
comms_messaging_outbound_send_success_total
```

如果指标系统允许低基数 peer 标签，可以按 peer 记录短 ID；否则避免直接使用完整 public key，防止指标基数失控。

日志至少包含：

```text
peer_node_id
stream_id
message_tag
message_size
stall_duration
```

建议只在发生超时、过期丢弃时记录 `debug` 或限频 `warn`，不要为每条正常消息打印日志。


## 9. 本地验证方案

### 9.1 构造慢 peer

启动两个节点：

```text
节点 A：出块节点
节点 B：连接成功，但故意停止读取或限制网络速度
```

让节点 A 持续产生 NewBlock 或广播测试消息。

修改前预期：

```text
A 对 B 的 outbound queue 持续增长
BroadcastTask / outbound 相关 inuse_space 持续增长
没有写入超时
连接长期不结束
```

修改后预期：

```text
超过 write stall timeout 后记录超时
当前 outbound session 结束
过期消息被丢弃
未过期消息按现有策略 retry
内存不再因为单个慢 peer 无限增长
```


### 9.2 pprof 验证

必须对比 `inuse_space`，不能只看累计分配量：

```powershell
go tool pprof -top -sample_index=inuse_space profile.pb.gz
go tool pprof -top -sample_index=alloc_space profile.pb.gz
```

重点观察：

- `BroadcastTask<S>::handle`
- `tower::util::oneshot::Oneshot::poll`
- `futures_util::stream::for_each::ForEach::poll`
- `prost::Message::encode_to_vec`
- messaging outbound queue 相关路径

通过标准：

```text
慢 peer 存在时，内存可以出现短期峰值；
写入超时触发并清理后，inuse_space 不再持续单调上涨。
```


## 10. 测试用例

至少增加以下测试：

1. sink 正常可写时，消息正常发送，不能误触发超时。
2. sink 的 `poll_ready` 长期 Pending 时，触发写入停滞超时。
3. sink 的 `poll_flush` 长期 Pending 时，触发写入停滞超时。
4. 写入超时后，当前 outbound handler 能结束。
5. handler 结束后，过期消息被丢弃。
6. handler 结束后，未过期消息进入 retry queue。
7. disconnect 发生时，仍然优先结束 handler。
8. Ping/Pong 被过期丢弃时，不会由 `reply_fail(Dropped)` 直接修改 `failed_pings`。


## 11. 风险与控制

### 11.1 超时设置过短

可能错误断开高延迟但正常的 peer。

控制方式：

- 初始使用 60 秒。
- 观察正常环境的最大 flush 延迟。
- 允许通过配置调整。


### 11.2 重连风暴

慢 peer 可能不断：

```text
连接 → 写入超时 → 重连 → 再次超时
```

控制方式：

- 复用现有连接重试退避。
- 保留消息过期清理。
- 后续增加每个 peer 的重试次数或退避上限。


### 11.3 吞吐下降

如果采用逐条 `sink.send()`，每条消息都 flush，可能降低吞吐量。

控制方式：

- 优先扩展现有 `Forward`。
- 保留当前批量 poll 和 flush 逻辑。


### 11.4 本地拥塞被误判为 peer 问题

如果 Tokio runtime 因 RandomX 或区块验证严重饥饿，也可能触发写入超时。

控制方式：

- 同时记录 runtime、区块验证任务和 outbound queue 指标。
- 超时先断开连接，不立即 ban peer。
- 不因为 `SendFailReason::Dropped` 直接增加 peer failcount。


## 12. 修改后的副作用总结

增加写入停滞超时能够避免单个慢 peer 永久阻塞发送链路，但它会改变原有网络行为。实施前必须明确以下副作用。

### 12.1 可能误断开慢但正常的 peer

以下情况可能短时间没有写入进展：

- peer 正在高负载处理区块
- 网络短时间拥堵或高丢包
- 高延迟网络
- 本机 Tokio runtime 因区块验证或 RandomX 任务调度不足
- 大消息写入耗时较长

如果超时阈值过短，会产生：

```text
正常但较慢的 peer
    ↓
被判定为写入停滞
    ↓
当前 outbound session 被结束
    ↓
连接重建和消息 retry
```

控制方式：

- 检测“连续没有任何写入进展”，不要限制整条消息总发送时间。
- 第一阶段只告警，不自动断开。
- 自动断开阈值建议从 5 分钟开始。
- 确认正常环境最大停滞时间后再逐步缩短。


### 12.2 可能将压力转移到 retry queue

当前 retry queue 也是无界队列。

写入停滞后如果把全部未过期消息放入 retry queue，异常 peer 可能不断循环：

```text
连接
    ↓
写入停滞
    ↓
消息进入 retry queue
    ↓
重新连接
    ↓
再次写入停滞
```

控制方式：

- retry 前必须检查 `is_expired()`。
- 复用或增加连接重试退避。
- 记录每个 peer 的 retry 次数。
- 后续增加最大 retry 次数或 retry queue 上限。


### 12.3 消息可能重复发送

写入超时时，无法保证消息完全没有发送到对端。

例如：

```text
消息已经写入本机 yamux/TCP buffer
    ↓
flush 长时间没有完成
    ↓
本机触发超时并 retry
    ↓
对端之后收到原消息和 retry 消息
```

控制方式：

- NewBlock 等传播类消息依赖现有 DHT 去重。
- 请求/响应类消息不能默认安全 retry，需要按消息类型判断。
- 第一阶段保持当前 retry 行为，不新增激进 retry。


### 12.4 消息可能丢失

如果当前阻塞消息超时后直接标记失败，并且上层没有等待或处理失败结果，该消息可能丢失。

推荐策略：

| 消息状态 | 推荐处理 |
| --- | --- |
| 已过期消息 | 直接丢弃 |
| 当前阻塞且未确认是否已发送 | 保守标记失败；是否 retry 后续按消息类型决定 |
| 队列中未过期消息 | 进入现有 retry queue |
| 队列中过期消息 | 直接丢弃 |


### 12.5 Ping/Pong 的 `failed_pings` 可能增加

写入停滞导致 Ping 没有真正到达 peer 时：

```text
Ping nonce 已加入 inflight_pings
    ↓
Ping 未成功发送
    ↓
无法收到 Pong
    ↓
inflight Ping 到期
    ↓
failed_pings + 1
```

需要注意：

- `reply_fail(SendFailReason::Dropped)` 本身不会直接增加 `failed_pings`。
- `failed_pings` 增加是因为没有收到对应 Pong。
- 如果是本机整体拥塞，可能错误认为多个 peer 都不健康。

控制方式：

- 写入停滞后只结束当前连接，不直接 ban peer。
- 同时观察 `failed_pings` 和 outbound write stall 指标。
- 如果大量 peer 同时 write stall，应优先排查本机 runtime 或 CPU 压力。


### 12.6 逐条 `sink.send()` 可能降低吞吐量

如果用逐条发送替换当前 `Forward`：

```rust
sink.send(message).await
```

可能每条消息都会执行 flush，降低批量发送吞吐。

控制方式：

- 优先扩展现有 `Forward`。
- 保留当前批量 `poll_ready`、`start_send` 和 `poll_flush` 行为。
- 不在第一阶段切换为逐条 `sink.send()`。


### 12.7 清理逻辑错误可能产生新的滞留

写入超时后必须确保：

- 当前消息 reply 被完成
- `messages_rx` 被关闭
- 队列中的剩余消息被 drain
- 过期消息被丢弃
- 未过期消息正确进入 retry
- sink、stream 和连接 handle 被释放

否则可能产生：

- oneshot 永久等待
- 消息重复 reply
- 消息无声丢失
- 新的 future 或连接资源滞留


## 13. 推荐的具体修改方案

建议不要一次性启用自动断连。按以下阶段实施，能够先证明根因，再逐步改变网络行为。

### 第一阶段：只观测，不改变连接行为

修改位置：

- `comms/core/src/protocol/messaging/forward.rs`
- `comms/core/src/protocol/messaging/outbound.rs`
- `comms/core/src/protocol/messaging/metrics.rs`

修改内容：

1. 在 `Forward` 中记录最近一次取得写入进展的时间。
2. 分别观测 `poll_ready` 和 `poll_flush` 持续 Pending 的时间。
3. 超过告警阈值时，只记录限频日志和 Prometheus 指标。
4. 不返回错误，不结束连接，不改变 retry 行为。

建议告警阈值：

```text
60 秒
```

必须增加的指标：

```text
comms_messaging_outbound_write_stall_warning_total
comms_messaging_outbound_current_stall_seconds
comms_messaging_outbound_write_progress_total
```

验证目标：

```text
确认内存上涨期间是否存在某个或多个 peer 长时间没有写入进展。
```


### 第二阶段：启用写入停滞超时

在第一阶段确认存在长时间写入停滞后，再增加可配置超时。

建议配置：

```text
outbound_write_stall_timeout = 5 分钟
```

修改行为：

```text
连续超过 timeout 没有任何写入进展
    ↓
Forward 返回 WriteStalled
    ↓
结束当前 outbound session
    ↓
触发现有队列 drain 和 retry 逻辑
```

要求：

- 只断开或结束当前连接，不 ban peer。
- 记录 peer、stream_id、停滞时间和待发送消息大小。
- 写入取得任何有效进展后重置计时器。


### 第三阶段：启用发送侧过期清理

原始代码没有 outbound 过期清理，需要保留当前工作区中的实验性修改：

```text
DHT expires
    ↓
传递到 OutboundMessage.expires_at
    ↓
发送前检查 is_expired()
    ↓
retry 前检查 is_expired()
```

处理规则：

```text
已过期消息 → reply_fail(Dropped) → 丢弃
未过期消息 → 正常发送或进入 retry
```

这一步负责在写入停滞 session 被终止后，释放已经过期的队列消息。


### 第四阶段：修正发送结果语义

当前代码在消息交给 `Forward` 前调用 `reply_success()`。后续应修改为：

```text
消息真正写入 sink 成功
    ↓
再调用 reply_success()
```

同时区分：

- `DroppedExpired`
- `WriteStalled`
- `PeerDialFailed`
- `ConnectionClosed`

这一步会影响发送状态语义，建议在写入停滞问题得到验证后再实施。


### 第五阶段：进一步限制异常 peer 的内存占用

1. 对慢 peer 合并或丢弃旧 NewBlock。
2. 给 retry queue 增加上限或最大重试次数。
3. 给 per-peer queue 增加按消息类型的溢出策略。
4. 再评估是否限制 `broadcast.rs` 的 fan-out 并发。


## 14. 推荐代码修改顺序

推荐按以下顺序提交代码，避免单次改动过大：

### 修改 1：观测写入停滞

```text
Forward 增加最近写入进展时间
    +
增加 write stall Prometheus 指标
    +
超过 60 秒只记录告警
```

不改变连接和发送行为，风险最低。


### 修改 2：写入停滞后结束 session

```text
增加可配置 outbound_write_stall_timeout
    +
默认先设置 5 分钟
    +
超时后返回 WriteStalled
    +
结束当前 outbound session
```

需要覆盖正常发送、慢 sink、disconnect 和 timeout 测试。


### 修改 3：清理过期队列消息

```text
保留 OutboundMessage.expires_at
    +
发送前检查
    +
retry 前检查
```

该修改保证 session 终止后，已经失去价值的消息不会继续占用内存或反复 retry。


### 修改 4：控制 retry 循环

在确认异常 peer 会频繁重连后，再增加：

```text
重试退避
    +
最大重试次数
    +
retry queue 观测或上限
```


## 15. 最终建议

当前最值得优先实施的不是单纯把 `.unordered()` 改成有限并发，也不是只依赖消息 TTL。

建议先实现低风险观测版本：

```text
按 peer 记录写入进展
    ↓
超过 60 秒无进展时记录指标和限频日志
    ↓
确认 pprof 内存上涨与 write stall 同时发生
```

确认根因后，再启用：

```text
5 分钟写入停滞超时
    ↓
结束当前 outbound session
    ↓
丢弃已过期消息
    ↓
未过期消息进入现有 retry
```

该实施顺序能够直接验证 `test/doc/leak.md` pprof 显示的 outbound future 长期存活问题，同时降低误断连接、重复发送和 retry 风暴风险。
