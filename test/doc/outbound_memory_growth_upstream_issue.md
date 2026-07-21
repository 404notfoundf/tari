# Possible outbound memory growth on a mining base node

## Summary

We are observing continuous RSS growth on a mining/base node that regularly accepts and propagates newly mined blocks.

The node continues operating normally:

- blocks are successfully submitted and accepted;
- the node continues mining;
- other network nodes receive the new blocks;
- no complete network outage is observed.

However, heap profiling shows that live memory attributed to the outbound broadcast path grows significantly over time.

We would like to confirm whether this may be caused by outbound messages being retained for slow or stalled peers.

## Profiling evidence

Earlier profile:

![Earlier outbound profile](img_6.png)

The live memory attributed to:

```text
tari_comms_dht::outbound::broadcast::BroadcastTask<S>::handle
```

was approximately `326.50MB`.

Later profile:

![Later outbound profile](img_7.png)

The same outbound call path increased to approximately `2.01GB`.

The later profile also shows approximately `1.94GB` attributed through:

```text
futures_util::stream::for_each::ForEach::poll
```

We also observed growing live allocations originating from Protobuf message encoding.

We understand that these profiles show where the memory was allocated, but the encoded messages may currently be retained by downstream queues or network-writing futures.

## Relevant block propagation path

When a miner submits a valid block, the node adds it and propagates a `NewBlock` message:

```text
gRPC submit_block
    → LocalNodeCommsInterface::submit_block
    → add block
    → OutboundNodeCommsInterface::propagate_block
    → handle_outbound_block
    → OutboundMessageRequester::propagate
    → BroadcastTask::handle
    → SerializeMiddleware::call
    → per-peer outbound messaging queue
    → Forward
    → Yamux/TCP
```

The default DHT propagation factor is currently `20`, so one accepted block may produce messages for multiple peers.

Each target peer receives an independently encoded DHT envelope:

```rust
let body = Bytes::from(envelope.to_encoded_bytes());
```

## Areas of concern

### Broadcast fan-out

`BroadcastTask::handle` concurrently processes all messages selected for the propagation round:

```rust
self.service
    .call_all(stream::iter(messages))
    .unordered()
    .filter_map(|result| future::ready(result.err()))
    .for_each(...)
    .await;
```

### Unbounded queues

The outbound path contains several unbounded queues, including:

- the base-node outbound block queue;
- the DHT outbound request queue;
- each peer's outbound messaging queue;
- the messaging retry queue.

In particular, each peer has an unbounded outbound queue:

```rust
let (msg_tx, msg_rx) = mpsc::unbounded_channel();
```

### Slow or stalled peer writes

Messages are written to each peer through:

```rust
Forward::new(stream, sink).await
```

`Forward` waits for the sink using `poll_ready` and `poll_flush`.

Our current hypothesis is that one or more connections may remain established while their messaging substream is unable to make sufficient write progress. New block propagation messages could then continue accumulating for those peers.

This would explain why:

- the node and most peers continue operating normally;
- the network still receives newly mined blocks;
- only one or a few slow peers may retain messages;
- encoded outbound buffers remain live and RSS continues growing.

## Current analysis

Based on the heap profiles and the current outbound implementation, we believe the memory growth is most likely caused by encoded outbound messages accumulating in one or more per-peer messaging queues.

The likely sequence is:

1. A successfully mined block triggers `NewBlock` propagation.
2. DHT propagation selects multiple peers, with a default `propagation_factor` of `20`.
3. A separate DHT envelope buffer is encoded for every selected peer.
4. The encoded message is moved into that peer's unbounded outbound queue.
5. If a peer remains connected but its messaging substream stops making progress in `poll_ready` or `poll_flush`, its outbound handler does not exit.
6. New block messages continue being appended to that peer's queue.
7. The encoded buffers remain live, causing RSS and allocations attributed to `BroadcastTask::handle` and Protobuf encoding to continuously increase.

This does not require every peer to be blocked. Other peers may continue receiving blocks normally, which explains why the node continues mining and the network still observes the new blocks.

The existing 10-second outbound pipeline timeout does not appear to release messages that have already been serialized and moved into a per-peer messaging queue. Additionally, `reply_success()` is called when the message is dequeued and handed to `Forward`, before the sink has necessarily completed flushing it to the network.

The per-peer outbound queues and retry queue are currently unbounded. We could not identify an existing write-stall timeout in `Forward` or an existing mechanism that prevents queue growth when a connected peer stops consuming messages.

## Request for confirmation

Could you please confirm whether this analysis matches the intended outbound messaging behaviour?

In particular:

1. Can a messaging substream remain blocked in `poll_ready` or `poll_flush` while the peer connection remains active?
2. Is there an existing slow-peer or write-stall cleanup mechanism that we may have missed?
3. Are unbounded per-peer and retry queues expected to remain safe under this condition?
4. Is there a recommended way to identify the affected peer and observe its outbound queue/write progress?

## Additional context

- The issue is more visible on the mining node than on ordinary nodes.
- A successful block is produced approximately every five minutes.
- Seeing the block on the network confirms that some peers received it, but does not confirm that every selected peer completed its send.
- We are currently treating `BroadcastTask` and Protobuf encoding as allocation sources; the final retention point may be further downstream.

Any guidance on expected outbound queue behaviour, slow-peer handling, or additional metrics/logging that would help confirm the retention point would be appreciated.
