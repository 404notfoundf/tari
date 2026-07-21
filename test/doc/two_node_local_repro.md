# 双节点本地复现 Outbound 堆积说明

本文档用于在本地启动两个 `minotari_node` 节点，观察 `OutboundMessage` 相关指标，验证是否存在发送阻塞、队列积压和内存持续上涨问题。

适用目标：

- 先验证两个节点是否能互连
- 再验证慢节点场景下 `outbound_pending_messages` 是否持续增长
- 再判断问题更偏向 `broadcast`、`retry queue`、`per-peer queue` 还是 `Forward::new(...)` 写出阻塞

---

## 1. 前置条件

需要具备以下环境：

- Windows 本机
- 已安装 Rust 工具链
- 已安装 MSVC Build Tools，确保 `link.exe` 可用
- 当前仓库路径：`D:\rust\project\t\tari`

如果 `cargo build` / `cargo test` 报缺少 `link.exe`，先补齐 Visual Studio Build Tools，否则无法本地编译验证。

---

## 2. 目标思路

双节点复现不是为了完整跑链，而是为了构造下面这个最小场景：

- 节点 A 可以持续发消息
- 节点 B 可以接入，但处理变慢
- A 的发送路径因此出现积压
- 通过 Prometheus 指标确认积压是否持续扩大

重点观察指标：

- `comms::messaging::outbound_pending_messages`
- `comms::messaging::retry_queue_messages`
- `comms::messaging::outbound_queue_enqueue_count`
- `comms::messaging::outbound_queue_dequeue_count`

判断规则：

- `outbound_pending_messages` 持续上涨：发送链路 drain 不动
- `retry_queue_messages` 持续上涨：断连/重试路径也在积压
- `enqueue_count - dequeue_count` 持续扩大：生产长期大于消费

---

## 3. 编译带 metrics 的节点

在仓库根目录执行：

```powershell
cargo build --bin minotari_node --features metrics
```

如果你想先用 release：

```powershell
cargo build --release --bin minotari_node --features metrics
```

下文默认使用 debug 二进制：

```text
.\target\debug\minotari_node.exe
```

---

## 4. 准备两个独立目录

创建两个独立节点目录，避免配置、数据库和 identity 相互污染。

```powershell
New-Item -ItemType Directory -Force D:\tmp\tari-node-a | Out-Null
New-Item -ItemType Directory -Force D:\tmp\tari-node-b | Out-Null
```

---

## 5. 初始化两个节点

分别执行：

```powershell
.\target\debug\minotari_node.exe --network localnet --base-path D:\tmp\tari-node-a --init
```

```powershell
.\target\debug\minotari_node.exe --network localnet --base-path D:\tmp\tari-node-b --init
```

执行完成后，目录结构通常会包含：

```text
D:\tmp\tari-node-a\localnet\config\
D:\tmp\tari-node-a\localnet\data\
D:\tmp\tari-node-a\localnet\logs\
```

同理会有 `D:\tmp\tari-node-b\localnet\...`

---

## 6. 启动节点 A

先启动 A，开放一个固定的 TCP 地址和独立 metrics 端口。

```powershell
.\target\debug\minotari_node.exe --network localnet --base-path D:\tmp\tari-node-a `
  -p "metrics.server_bind_address=127.0.0.1:5577" `
  -p "base_node.p2p.public_addresses=/ip4/127.0.0.1/tcp/18141"
```

建议单独开一个终端窗口运行 A，不要立即关闭。

---

## 7. 获取节点 A 的 public_key

启动 B 前，需要先拿到 A 的 `public_key`。

有两种方式。

### 方式一：从 identity 文件读取

最稳的办法是直接看身份文件：

```powershell
Get-Content D:\tmp\tari-node-a\localnet\config\base_node_id.json
```

这里通常会包含：

- `public_key`
- `node_id`
- `public_addresses`

记下 `public_key` 的完整值。

### 方式二：从日志中获取

如果启动日志里打印了节点身份，也可以直接从日志里搜：

```powershell
Get-ChildItem D:\tmp\tari-node-a\localnet\logs
```

然后查看日志文件中是否包含：

- `public_key`
- `node_id`
- `identity`

如果用控制台直接启动，也可以在终端里搜这些关键字。

---

## 8. 启动节点 B，并把 A 设为 seed

将上一步拿到的 A 的 `public_key` 替换到下面命令中：

```powershell
.\target\debug\minotari_node.exe --network localnet --base-path D:\tmp\tari-node-b `
  -p "metrics.server_bind_address=127.0.0.1:5578" `
  -p "base_node.p2p.public_addresses=/ip4/127.0.0.1/tcp/18142" `
  -p "localnet.p2p.seeds.peer_seeds=<A的public_key>::/ip4/127.0.0.1/tcp/18141"
```

这一步的目标是：

- B 启动后能主动知道 A
- A 和 B 建立 P2P 连接

---

## 9. 验证两个节点是否已连通

优先看日志。

如果连接建立成功，通常会看到：

- peer dial 成功
- connected peer
- connection established
- messaging substream 建立

如果连接没有建立，先检查：

- 端口 `18141`、`18142` 是否被占用
- `public_addresses` 是否拼写正确
- B 使用的 `public_key` 是否正确
- 是否确实使用了 `localnet`

---

## 10. 验证 metrics 是否可访问

节点 A：

```powershell
curl http://127.0.0.1:5577/metrics
```

节点 B：

```powershell
curl http://127.0.0.1:5578/metrics
```

只看 messaging 相关指标：

```powershell
curl http://127.0.0.1:5577/metrics | findstr "comms::messaging"
```

```powershell
curl http://127.0.0.1:5578/metrics | findstr "comms::messaging"
```

如果指标能看到，说明 Prometheus 暴露已正常工作。

---

## 11. 先做基础联通验证

在没有人为制造慢节点之前，先确认基线状态正常。

此时预期应该是：

- `outbound_pending_messages` 偶尔波动，但会回落
- `retry_queue_messages` 长时间接近 0
- `enqueue_count - dequeue_count` 不会一直扩大

如果这一步都不正常，就先不要继续做慢节点实验，先排查基本连通性和节点配置。

---

## 12. 如何制造“慢节点”

要复现你关心的问题，关键不是断开连接，而是让对端“连着但很慢”。

因为：

- 断开连接更容易把问题导向 `retry queue`
- 连着但不 drain，才更容易逼出 `Forward::new(...)` / sink 写出阻塞

建议优先使用以下方式。

### 方式一：代码里临时加延迟

在 B 上临时加一个 debug patch，在消息发送或消费附近插入 `sleep`。

更适合的位置：

- `comms/core/src/protocol/messaging/outbound.rs`
- 或接收侧实际消费消息的路径

如果目标只是让对端变慢，这种方式最容易稳定复现。

### 方式二：让接收侧不消费或很慢消费

这更接近真实问题：

- substream 是建立的
- 连接没有断
- 但消息 drain 不出去

如果能做到这一点，最容易验证：

- TTL 为什么只能清一部分消息
- 为什么 `Forward::new(...)` 卡住后，内存仍会继续上涨

---

## 13. 如何施加持续消息压力

仅仅让两个节点互连不够，还要让 A 持续产生 outbound 流量。

可选方式：

- 持续触发 `Ping/Pong`
- 持续触发 `Join` / `Propagate Join`
- 持续触发 `Discovery`
- 人工构造更高频的 direct / broadcast 消息

如果只是想先看趋势，优先用项目里现成会周期产生的 liveness 消息即可。

相关配置参考：

- `base_node.metadata_auto_ping_interval`

如果要增加消息频率，可以适当调小这个值，例如：

```powershell
-p "base_node.metadata_auto_ping_interval=5"
```

这样会更容易在短时间内观察到积压。

---

## 14. 实时观察指标

建议盯 A 的 metrics，因为你主要关心发送方是否在积压。

持续执行：

```powershell
curl http://127.0.0.1:5577/metrics | findstr "outbound_pending_messages retry_queue_messages outbound_queue_enqueue_count outbound_queue_dequeue_count"
```

重点观察：

### 场景一：正常波动

表现：

- `outbound_pending_messages` 有涨有跌
- `enqueue_count` 和 `dequeue_count` 大体同步
- `retry_queue_messages` 不会一直涨

说明：

- 系统只是有瞬时波动
- 不足以证明存在持续堆积

### 场景二：慢节点导致持续积压

表现：

- `outbound_pending_messages` 持续上升
- `enqueue_count - dequeue_count` 持续扩大
- `retry_queue_messages` 可能上升，也可能不上升

说明：

- 下游发送速度长期小于上游生产速度
- 发送链路正在积压

### 场景三：更像断连重试问题

表现：

- `retry_queue_messages` 持续上升
- 日志中反复有 reconnect / substream error / dial error

说明：

- 问题更偏向断连重试，而不是纯慢消费

---

## 15. 如何配合内存趋势一起看

只看 metrics 还不够，建议同时观察进程内存。

Windows 下可以先用：

```powershell
Get-Process minotari_node | Select-Object Id, ProcessName, WorkingSet64
```

如果出现下面组合：

- `outbound_pending_messages` 持续涨
- `enqueue_count - dequeue_count` 持续扩大
- `WorkingSet64` 也持续涨

这说明当前更像是发送链路 backlog 在吃内存，而不是单纯 allocator 假象。

---

## 16. 当前方案的局限

即使复现成功，也要注意这几点：

- `Ping/Pong/Join/Discovery` 现在已经加了 TTL
- TTL 只能清理已经 dequeue 到检查点的消息
- 如果卡在 `Forward::new(...)` 内部，TTL 不能直接打断当前阻塞
- `retry queue` 和 `per-peer queue` 仍然是无界的

所以如果你观察到：

- 内存持续涨
- metrics 也持续涨

那么下一步就不应继续只调 TTL，而应重点处理：

- `Forward::new(...)` 阻塞
- `retry queue` 无界
- `per-peer queue` 无界

---

## 17. 推荐的最小验证顺序

建议按这个顺序执行：

1. 编译 `minotari_node --features metrics`
2. 初始化 A、B 两个目录
3. 启动 A
4. 从 `base_node_id.json` 读取 A 的 `public_key`
5. 启动 B，并把 A 配成 `peer_seeds`
6. 先确认两个节点已连通
7. 确认 `/metrics` 可访问
8. 先观察基线指标
9. 再人为制造 B 变慢
10. 同时看 A 的 metrics 和进程内存

---

## 18. 结论

双节点本地复现的关键不在于“把两个节点跑起来”，而在于：

- A 持续发送
- B 连着但很慢
- A 的发送路径出现可持续积压

只要这三点同时成立，你就能比较可靠地验证：

- 现在的 TTL 机制能清掉多少旧消息
- 哪些堆积发生在 `retry queue`
- 哪些堆积发生在 `per-peer queue`
- 是否更像卡在 `Forward::new(...)` 的写出阶段
