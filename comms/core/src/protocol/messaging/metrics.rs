//  Copyright 2021, The Tari Project
//
//  Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
//  following conditions are met:
//
//  1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
//  disclaimer.
//
//  2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
//  following disclaimer in the documentation and/or other materials provided with the distribution.
//
//  3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
//  products derived from this software without specific prior written permission.
//
//  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
//  INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
//  DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//  SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//  SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
//  WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
//  USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use once_cell::sync::Lazy;
use tari_metrics::{IntCounter, IntGauge};

pub fn num_sessions() -> IntGauge {
    static METER: Lazy<IntGauge> = Lazy::new(|| {
        tari_metrics::register_int_gauge(
            "comms::messaging::num_sessions",
            "The number of active messaging sessions",
        )
        .unwrap()
    });

    METER.clone()
}

pub fn outbound_message_count() -> IntCounter {
    static METER: Lazy<IntCounter> = Lazy::new(|| {
        tari_metrics::register_int_counter(
            "comms::messaging::outbound_message_count",
            "The number of handshakes per peer",
        )
        .unwrap()
    });

    METER.clone()
}

pub fn inbound_message_count() -> IntCounter {
    static METER: Lazy<IntCounter> = Lazy::new(|| {
        tari_metrics::register_int_counter(
            "comms::messaging::inbound_message_count",
            "The number of handshakes per peer",
        )
        .unwrap()
    });

    METER.clone()
}

pub fn error_count() -> IntCounter {
    static METER: Lazy<IntCounter> =
        Lazy::new(|| tari_metrics::register_int_counter("comms::messaging::errors", "The number of errors").unwrap());

    METER.clone()
}

pub fn outbound_queue_enqueue_count() -> IntCounter {
    static METER: Lazy<IntCounter> = Lazy::new(|| {
        tari_metrics::register_int_counter(
            "comms::messaging::outbound_queue_enqueue_count",
            "The total number of messages enqueued into per-peer outbound queues",
        )
        .unwrap()
    });
    METER.clone()
}

pub fn outbound_queue_dequeue_count() -> IntCounter {
    static METER: Lazy<IntCounter> = Lazy::new(|| {
        tari_metrics::register_int_counter(
            "comms::messaging::outbound_queue_dequeue_count",
            "The total number of messages dequeued from per-peer outbound queues",
        )
        .unwrap()
    });
    METER.clone()
}

pub fn outbound_pending_messages() -> IntGauge {
    static METER: Lazy<IntGauge> = Lazy::new(|| {
        tari_metrics::register_int_gauge(
            "comms::messaging::outbound_pending_messages",
            "The current number of messages waiting in per-peer outbound queues",
        )
        .unwrap()
    });
    METER.clone()
}

pub fn retry_queue_messages() -> IntGauge {
    static METER: Lazy<IntGauge> = Lazy::new(|| {
        tari_metrics::register_int_gauge(
            "comms::messaging::retry_queue_messages",
            "The current number of messages waiting in the outbound retry queue",
        )
        .unwrap()
    });
    METER.clone()
}

pub fn active_outbound_queues() -> IntGauge {
    static METER: Lazy<IntGauge> = Lazy::new(|| {
        tari_metrics::register_int_gauge(
            "comms::messaging::active_outbound_queues",
            "The current number of active per-peer outbound queues",
        )
        .unwrap()
    });
    METER.clone()
}

pub fn outbound_queue_abandoned_count() -> IntCounter {
    static METER: Lazy<IntCounter> = Lazy::new(|| {
        tari_metrics::register_int_counter(
            "comms::messaging::outbound_queue_abandoned_count",
            "The total number of queued messages abandoned when an outbound handler exits with an error",
        )
        .unwrap()
    });
    METER.clone()
}
