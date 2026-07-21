`OutboundMessage` / outbound queue 本地观测与测试说明

本文档用于本地验证：

1. 新增的 Prometheus 指标是否生效
2. `OutboundMessage` 是否因为发送背压或断连重试而堆积
3. 如何在不依赖线上环境的情况下重复验证


## 1. 本次新增的指标

代码位置：

- `comms/core/src/protocol/messaging/metrics.rs`
- `comms/core/src/protocol/messaging/protocol.rs`
- `comms/core/src/protocol/messaging/outbound.rs`

新增 Prometheus 指标如下：

- `comms::messaging::outbound_queue_enqueue_count`
  - 含义：累计入队到 per-peer outbound queue 的消息数

- `comms::messaging::outbound_queue_dequeue_count`
  - 含义：累计从 per-peer outbound queue 取出的消息数

- `comms::messaging::outbound_pending_messages`
  - 含义：当前仍然滞留在 per-peer outbound queue 中的消息数

- `comms::messaging::retry_queue_messages`
  - 含义：当前 retry queue 中的消息数

- `comms::messaging::active_outbound_queues`
  - 含义：当前活跃的 per-peer outbound queue 数量


## 2. 本地测试代码

本次补充了 3 个本地测试，位置：

- `comms/core/src/protocol/messaging/test.rs`

测试名称：

- `send_message_updates_prometheus_metrics`
  - 验证正常发送时 enqueue/dequeue/active queue 指标会更新

- `send_message_disconnect_updates_retry_metrics`
  - 验证断连后 retry queue 指标会上升

- `send_message_backpressure_increases_pending_metrics`
  - 验证对端不消费时，发送背压会导致 `outbound_pending_messages` 上升


## 3. 运行前准备

### 3.1 Rust 环境

需要本地可以正常运行 Rust 编译和测试命令。

### 3.2 Windows 工具链

如果你在 Windows 上运行，且之前遇到过：

```text
link.exe not found
```

说明本机缺少 MSVC C++ Build Tools。

需要安装：

- Visual Studio Build Tools
- C++ build tools / MSVC toolchain

否则 `cargo check` / `cargo test` 无法完成。


## 4. 最推荐的本地验证方式

优先使用单元测试直接验证，不需要先起完整节点，也不需要先看线上环境。

为了避免 Prometheus 全局 registry 被并行测试干扰，建议所有测试都带：

```powershell
-- --test-threads=1 --nocapture
```


## 5. 逐个运行测试

### 5.1 验证基础指标会更新

```powershell
cargo test -p tari_comms send_message_updates_prometheus_metrics --features metrics -- --test-threads=1 --nocapture
```

预期：

- 测试通过
- 表明正常发送路径会更新：
  - `outbound_queue_enqueue_count`
  - `outbound_queue_dequeue_count`
  - `outbound_pending_messages`
  - `active_outbound_queues`


### 5.2 验证断连后 retry queue 指标

```powershell
cargo test -p tari_comms send_message_disconnect_updates_retry_metrics --features metrics -- --test-threads=1 --nocapture
```

预期：

- 测试通过
- 表明断连后消息会进入 retry queue
- `retry_queue_messages` 会增加


### 5.3 验证发送背压导致 pending 增长

```powershell
cargo test -p tari_comms send_message_backpressure_increases_pending_metrics --features metrics -- --test-threads=1 --nocapture
```

预期：

- 测试通过
- 表明对端不消费时，会形成发送背压
- `outbound_pending_messages` 上升
- `outbound_queue_enqueue_count > outbound_queue_dequeue_count`

这一条测试最关键，因为它最接近“`OutboundMessage` 堆积导致内存上涨”的场景。


## 6. 一次性运行这 3 个测试

如果只想跑本次新增的 metrics 相关测试，可以用：

```powershell
cargo test -p tari_comms send_message_ --features metrics -- --test-threads=1 --nocapture
```

说明：

- 这个命令会匹配以 `send_message_` 开头的测试
- 如果后续新增同前缀测试，也可能一起被跑到

如果想更严格，建议还是按第 5 节逐条执行。


## 7. 如何理解测试结果

### 7.1 正常发送

如果正常发送测试通过，说明：

- 指标已经成功接入 Prometheus registry
- enqueue / dequeue / active queue 的观测点有效


### 7.2 断连重试

如果断连测试通过，说明：

- 断连后剩余消息确实会被移动到 retry queue
- `retry_queue_messages` 可以用于观测这类滞留


### 7.3 背压堆积

如果背压测试通过，说明：

- 对端不消费时，发送端队列会堆积
- `outbound_pending_messages` 能直接反映这种堆积

如果线上内存上涨时，看到：

- `outbound_pending_messages` 持续上涨不回落
- `outbound_queue_enqueue_count - outbound_queue_dequeue_count` 持续扩大

那么就很像是 outbound queue 阻塞或发送吞吐跟不上，而不是单纯的 Rust 对象泄露。


## 8. 如果要在运行中的节点里看 Prometheus

单元测试之外，如果想在节点进程里通过 HTTP 查看指标，需要：

### 8.1 配置 metrics 服务

配置文件位置可参考：

- `common/config/presets/a_common.toml`

打开配置：

```toml
[metrics]
server_bind_address = "127.0.0.1:5577"
```

### 8.2 使用带 metrics feature 的二进制运行

```powershell
cargo run -p minotari_node --features metrics
```

### 8.3 拉取指标

```powershell
curl http://127.0.0.1:5577/metrics
```

筛选本次新增指标：

```powershell
curl http://127.0.0.1:5577/metrics | findstr "comms::messaging"
```


## 9. 最建议的验证顺序

推荐顺序如下：

1. 先跑 `send_message_updates_prometheus_metrics`
2. 再跑 `send_message_disconnect_updates_retry_metrics`
3. 最后跑 `send_message_backpressure_increases_pending_metrics`
4. 如果上述都通过，再去节点进程里开 `/metrics` 做运行态观测

这样可以先确认：

- 指标有没有接对
- retry 场景能不能观测
- 背压堆积能不能观测

确认这些都成立后，再上更复杂的本地联调或线上环境。


## 10. 一句话结论

本地最重要的验证命令是：

```powershell
cargo test -p tari_comms send_message_backpressure_increases_pending_metrics --features metrics -- --test-threads=1 --nocapture
```

如果这条测试稳定通过，就说明现在已经可以在本地直接验证：

- `OutboundMessage` 是否因为发送背压而堆积
- `outbound_pending_messages` 是否能准确反映这种问题
