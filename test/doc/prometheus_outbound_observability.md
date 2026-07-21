# Outbound Prometheus 观测说明

本文档说明如何通过 Prometheus 直观判断 outbound 队列是否积压、retry 是否持续增长，以及内存上涨是否与发送链路有关。

## 1. 本次增加的指标

所有指标都是全局聚合指标，不使用 peer ID 标签，避免高基数和额外内存开销。

| 指标 | 类型 | 含义 |
|---|---|---|
| `comms::messaging::outbound_queue_enqueue_count` | Counter | 累计进入 per-peer outbound queue 的消息数 |
| `comms::messaging::outbound_queue_dequeue_count` | Counter | 累计从 per-peer outbound queue 取出的消息数 |
| `comms::messaging::outbound_pending_messages` | Gauge | 当前仍在所有 per-peer outbound queue 中等待的消息数 |
| `comms::messaging::retry_queue_messages` | Gauge | 当前 retry queue 中等待的消息数 |
| `comms::messaging::active_outbound_queues` | Gauge | 当前活跃的 per-peer outbound queue 数量 |
| `comms::messaging::outbound_queue_abandoned_count` | Counter | outbound handler 异常退出时被直接释放的队列消息数 |

## 2. 开启 Metrics

编译时必须启用 `metrics` feature：

```powershell
cargo build -p minotari_node --release --features metrics
```

配置文件中开启 HTTP scrape 服务：

```toml
[metrics]
server_bind_address = "127.0.0.1:5577"
```

启动节点后访问：

```powershell
curl.exe http://127.0.0.1:5577/metrics
```

筛选 messaging 指标：

```powershell
curl.exe http://127.0.0.1:5577/metrics | Select-String "comms::messaging"
```

## 3. 最直观的判断方式

### 3.1 判断 per-peer 队列是否持续积压

重点查看：

```text
comms::messaging::outbound_pending_messages
```

判断：

- 长期接近 `0`：发送链路通常能够及时消费。
- 出块时短暂上涨后回落：通常正常。
- 持续上涨且不回落：存在慢 peer、发送阻塞或生产速度长期高于发送速度。

### 3.2 判断 retry 是否形成积压

重点查看：

```text
comms::messaging::retry_queue_messages
```

判断：

- 短暂上涨后回落：通常是连接抖动后的正常重试。
- 持续上涨：连接频繁断开、重试消费不及时或 peer 长期不可用。

### 3.3 判断 outbound handler 是否异常退出并丢弃队列

重点查看：

```text
comms::messaging::outbound_queue_abandoned_count
```

判断：

- 正常运行时应很少增长。
- 如果与内存上涨、连接错误同时快速增长，说明 outbound handler 经常异常退出，队列中的消息被直接释放。

### 3.4 判断积压是否集中发生

可计算：

```text
outbound_pending_messages / active_outbound_queues
```

该值表示平均每个活跃 peer queue 的积压量。

- active queue 数稳定，但平均积压持续上涨：少量或多个 peer 的消费速度不足。
- active queue 数快速上涨：节点正在同时向越来越多的 peer 建立 outbound handler。

## 4. 推荐 PromQL

Prometheus 实际暴露的指标通常会带 `tari_` namespace。请先在 `/metrics` 中确认最终名称，再替换以下示例名称。

### 当前 outbound 队列积压

```promql
tari_comms::messaging::outbound_pending_messages
```

### 当前 retry 队列积压

```promql
tari_comms::messaging::retry_queue_messages
```

### 每分钟入队速度

```promql
rate(tari_comms::messaging::outbound_queue_enqueue_count[5m])
```

### 每分钟出队速度

```promql
rate(tari_comms::messaging::outbound_queue_dequeue_count[5m])
```

### 生产速度减消费速度

```promql
rate(tari_comms::messaging::outbound_queue_enqueue_count[5m])
-
rate(tari_comms::messaging::outbound_queue_dequeue_count[5m])
```

长期大于 `0` 表示队列总体上持续积压。

如果 `outbound_queue_abandoned_count` 同时增长，需要把异常退出释放的消息也考虑进去；此时优先使用
`outbound_pending_messages` 判断当前实时积压。

### 平均每个活跃 peer queue 的积压

```promql
tari_comms::messaging::outbound_pending_messages
/
clamp_min(tari_comms::messaging::active_outbound_queues, 1)
```

### handler 异常退出时释放消息的速度

```promql
rate(tari_comms::messaging::outbound_queue_abandoned_count[5m])
```

## 5. 推荐告警

### outbound 队列持续上涨

```promql
deriv(tari_comms::messaging::outbound_pending_messages[15m]) > 0
and
tari_comms::messaging::outbound_pending_messages > 1000
```

持续 15 分钟后告警。

### retry 队列积压

```promql
tari_comms::messaging::retry_queue_messages > 500
```

持续 10 分钟后告警。

### outbound handler 异常释放消息

```promql
increase(tari_comms::messaging::outbound_queue_abandoned_count[10m]) > 0
```

## 6. 如何结合内存判断根因

如果 RSS 持续上涨，同时出现：

```text
outbound_pending_messages 持续上涨
enqueue rate > dequeue rate
```

则高度怀疑内存上涨来自 outbound per-peer queue 积压。

如果 RSS 持续上涨，同时出现：

```text
retry_queue_messages 持续上涨
```

则高度怀疑连接断开和重试链路正在保留消息。

如果 RSS 上涨，但上述队列指标始终稳定，则需要继续调查：

- Broadcast future 在途数量
- Protobuf 编码 buffer
- RandomX VM/cache
- 其他业务队列

## 7. 性能影响

本次指标只在消息入队、出队、retry 和 handler 生命周期变化时更新原子 Counter/Gauge：

- 不扫描队列。
- 不遍历 peer。
- 不增加定时任务。
- 不使用 peer ID 标签。
- 不记录消息内容。

因此性能影响较低，适合在线上长期启用。

由于没有使用 peer ID 标签，这些指标不能直接指出具体是哪个 peer 阻塞。确认出现积压后，再结合 outbound
错误日志和连接日志定位具体 peer，可以避免 Prometheus 因大量 peer 标签产生高基数开销。
