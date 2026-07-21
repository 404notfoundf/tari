# 慢节点 Debug 开关实施说明

本文档用于说明如何在本地通过一个仅 `debug` 或 feature 生效的开关，稳定制造“慢节点”场景，以便复现 `OutboundMessage` 积压、`retry queue` 积压和 `Forward::new(...)` 阻塞问题。

目标不是长期保留该逻辑，而是提供一个低风险、可控、可关闭的本地复现手段。

---

## 1. 目标

希望构造下面这种场景：

- 节点 A 持续发送消息
- 节点 B 仍保持连接
- 但 B 的消息处理或 drain 速度明显下降
- 从而让 A 的 outbound 路径出现积压

通过这种方式，本地可以更稳定地验证：

- `outbound_pending_messages` 是否持续上涨
- `retry_queue_messages` 是否增长
- `enqueue_count - dequeue_count` 是否持续扩大
- TTL 清理机制是否只清掉一部分消息
- 是否更像卡在 `Forward::new(...)` 写出阶段

---

## 2. 推荐原则

这类调试开关应满足以下原则：

- 默认关闭
- 只在本地 debug 或显式 feature 下启用
- 不影响 release 默认行为
- 尽量少改业务逻辑
- 可以快速开启、快速撤销

不建议直接把调试延迟长期写死在正式路径中。

---

## 3. 最推荐的实现方式

最稳的做法是增加一个“人工延迟消息处理”的 debug 开关。

形式上建议二选一：

- 编译期开关：`--features slow-peer-debug`
- 运行时开关：环境变量或配置项

如果只能选一个，优先建议：

- 编译期 feature + 运行时开关组合

也就是：

- feature 决定代码是否编译进来
- 环境变量决定本次进程是否实际启用

这样对正式环境最安全。

---

## 4. 最适合加延迟的位置

### 方案 A：接收侧消费前加延迟

这是最推荐的位置。

思路：

- 节点 B 已经建立连接
- 也能收到消息
- 但在真正处理 inbound 消息前先 `sleep`

优点：

- 最接近“对端很慢但没断连”
- 更容易制造发送方 backlog
- 更容易逼出 `Forward::new(...)` / sink 写出阻塞

适用目标：

- 验证发送方 A 的 `outbound_pending_messages` 是否持续上涨

### 方案 B：接收侧每条消息都 sleep

这和方案 A 类似，但更粗暴。

优点：

- 实现非常直接

缺点：

- 可能影响过大
- 所有 inbound 消息都会变慢

适合第一版快速验证。

### 方案 C：发送侧 outbound 前加延迟

不太推荐作为第一选择。

原因：

- 它会把问题变成“本地故意慢发”
- 不够接近真实的“对端慢导致发送阻塞”

只有在你想验证本地队列行为，而不关心真实对端 drain 时，才适合这样做。

---

## 5. 建议改动点

建议优先在接收侧 protocol 路径增加 debug 延迟。

可以排查以下候选文件：

- `comms/core/src/protocol/messaging/inbound.rs`
- `comms/dht/src/inbound/...`
- 真正开始消费 inbound message 的 service / task

第一版不要求挑最完美点，只要满足：

- B 收到消息后不会立即消费完
- 不会直接断开连接

就足够用于复现。

---

## 6. 推荐开关形式

### 方式一：环境变量

例如：

```text
TARI_DEBUG_SLOW_PEER_MS=500
```

含义：

- 每处理一条 inbound message，先 sleep 500ms

优点：

- 开关最方便
- 不用反复改配置文件

缺点：

- 需要加少量环境变量读取逻辑

### 方式二：配置项

例如：

```toml
base_node.debug_slow_peer_delay_ms = 500
```

优点：

- 更系统化

缺点：

- 配置链路改动更大

### 方式三：仅 debug 常量

例如：

```rust
#[cfg(debug_assertions)]
const DEBUG_SLOW_PEER_DELAY_MS: u64 = 500;
```

优点：

- 改动最小

缺点：

- 每次启停不灵活
- 需要反复改代码

第一版最推荐：

- feature + 环境变量

---

## 7. 建议行为

假设采用环境变量：

```text
TARI_DEBUG_SLOW_PEER_MS=500
```

逻辑建议：

1. 进程启动时读取该变量
2. 如果值存在且大于 0
3. 在 inbound 消费路径每条消息前 sleep 指定毫秒数

注意：

- sleep 只对当前节点生效
- 只在你指定的 B 节点上开启
- A 不应开启，否则会混淆结果

---

## 8. 推荐实验方式

用前面双节点文档中的步骤启动 A、B 后：

1. B 开启慢节点环境变量
2. A 正常启动并持续发消息
3. 观察 A 的 metrics

例如只让 B 变慢：

```powershell
$env:TARI_DEBUG_SLOW_PEER_MS="500"
.\target\debug\minotari_node.exe --network localnet --base-path D:\tmp\tari-node-b `
  -p "metrics.server_bind_address=127.0.0.1:5578" `
  -p "base_node.p2p.public_addresses=/ip4/127.0.0.1/tcp/18142" `
  -p "localnet.p2p.seeds.peer_seeds=<A的public_key>::/ip4/127.0.0.1/tcp/18141"
```

然后在 A 侧观察：

```powershell
curl http://127.0.0.1:5577/metrics | findstr "outbound_pending_messages retry_queue_messages outbound_queue_enqueue_count outbound_queue_dequeue_count"
```

---

## 9. 预期现象

如果慢节点复现成功，常见现象如下。

### 现象一：只出现轻微波动

表现：

- `outbound_pending_messages` 小幅波动后回落

说明：

- 当前消息压力不够
- 或 B 的延迟还不够大

建议：

- 增大 `TARI_DEBUG_SLOW_PEER_MS`
- 提高消息发送频率

### 现象二：发送侧持续积压

表现：

- `outbound_pending_messages` 持续上涨
- `enqueue_count - dequeue_count` 持续扩大

说明：

- 下游 drain 长期跟不上上游生产
- 已成功复现发送积压

### 现象三：更多进入 retry

表现：

- `retry_queue_messages` 持续上涨

说明：

- 连接断开或重试路径也参与了积压
- 问题不只是慢消费，还可能伴随连接不稳定

---

## 10. 为什么这个方式比直接断连更好

如果直接让 B 断连，看到的更可能是：

- `retry_queue_messages` 上升
- reconnect 行为增多

这会更偏向“断连重试问题”。

而“慢节点 debug 开关”的价值在于：

- 让连接保持存在
- 更容易验证 sink 写出阻塞
- 更容易逼近你当前怀疑的真实问题

所以它更适合用于定位：

- 为什么内存一直涨
- 为什么 TTL 清理不够
- 为什么 `Forward::new(...)` 可能迟迟不返回

---

## 11. 第一版建议

如果要做第一版实现，建议只做最小版本：

- 仅在 debug 或 feature 下编译
- 仅在节点 B 开启
- 仅加入“每条 inbound 消息前 sleep N ms”

不要第一版就同时加：

- 多种慢速模式
- 随机抖动
- 条件过滤特定消息类型

先把最小复现手段做稳定，再决定是否细化。

---

## 12. 结论

用于本地复现时，最合适的 debug 手段不是直接断网，也不是继续只调 TTL，而是：

- 保持 A 和 B 连通
- 只让 B 的消息消费变慢
- 用 metrics 观察 A 的发送侧 backlog

第一版最小实现建议：

- 增加一个仅 debug/feature 生效的慢节点开关
- 通过环境变量控制每条 inbound 消息前的延迟
- 配合 `two_node_local_repro.md` 文档一起使用
