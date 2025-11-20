use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use strum::IntoEnumIterator;

use rsnano_messages::{Message, MessageType};
use rsnano_network::{Channel, ChannelId, DeadChannelCleanupStep};
use rsnano_utils::{
    container_info::{ContainerInfo, ContainerInfoProvider},
    fair_queue::FairQueue,
    stats::{StatsCollection, StatsSource},
};

use crate::MessageCallback;

pub struct InboundMessageQueue {
    state: Mutex<State>,
    condition: Condvar,
    inbound_callback: Option<MessageCallback>,
    inbound_dropped_callback: Option<MessageCallback>,
    stats: MsgQueueStats,
}

impl InboundMessageQueue {
    pub fn new(max_queue: usize) -> Self {
        Self {
            state: Mutex::new(State {
                queue: FairQueue::new(move |_| max_queue, |_| 1),
                stopped: false,
            }),
            condition: Condvar::new(),
            inbound_callback: None,
            inbound_dropped_callback: None,
            stats: Default::default(),
        }
    }

    pub fn set_inbound_callback(&mut self, callback: MessageCallback) {
        self.inbound_callback = Some(callback);
    }

    pub fn set_inbound_dropped_callback(&mut self, callback: MessageCallback) {
        self.inbound_dropped_callback = Some(callback);
    }

    pub fn put(&self, message: Message, channel: Arc<Channel>) -> bool {
        let message_type = message.message_type();
        let added = self
            .state
            .lock()
            .unwrap()
            .queue
            .push(channel.channel_id(), (message.clone(), channel.clone()));

        if added {
            self.stats.processed.fetch_add(1, Ordering::Relaxed);
            self.stats.processed_type[message_type as usize].fetch_add(1, Ordering::Relaxed);
            self.condition.notify_one();
            if let Some(cb) = &self.inbound_callback {
                cb(channel.channel_id(), &message);
            }
        } else {
            self.stats.overfill.fetch_add(1, Ordering::Relaxed);
            self.stats.overfill_type[message_type as usize].fetch_add(1, Ordering::Relaxed);
            if let Some(cb) = &self.inbound_dropped_callback {
                cb(channel.channel_id(), &message);
            }
        }

        added
    }

    pub fn next_batch(
        &self,
        max_batch_size: usize,
    ) -> VecDeque<(ChannelId, (Message, Arc<Channel>))> {
        self.state.lock().unwrap().queue.next_batch(max_batch_size)
    }

    pub fn wait_for_messages(&self) {
        let state = self.state.lock().unwrap();
        if !state.queue.is_empty() {
            return;
        }
        drop(
            self.condition
                .wait_while(state, |s| !s.stopped && s.queue.is_empty()),
        )
    }

    pub fn size(&self) -> usize {
        self.state.lock().unwrap().queue.len()
    }

    /// Stop container and notify waiting threads
    pub fn stop(&self) {
        {
            let mut lock = self.state.lock().unwrap();
            lock.stopped = true;
        }
        self.condition.notify_all();
    }
}

impl Default for InboundMessageQueue {
    fn default() -> Self {
        Self::new(64)
    }
}

impl ContainerInfoProvider for InboundMessageQueue {
    fn container_info(&self) -> ContainerInfo {
        let guard = self.state.lock().unwrap();
        ContainerInfo::builder()
            .node("queue", guard.queue.container_info())
            .finish()
    }
}

pub struct InboundMessageQueueCleanup(Arc<InboundMessageQueue>);

impl InboundMessageQueueCleanup {
    pub fn new(queue: Arc<InboundMessageQueue>) -> Self {
        Self(queue)
    }
}

impl DeadChannelCleanupStep for InboundMessageQueueCleanup {
    fn clean_up_dead_channels(&self, dead_channel_ids: &[ChannelId]) {
        let mut guard = self.0.state.lock().unwrap();
        for channel_id in dead_channel_ids {
            guard.queue.remove(channel_id);
        }
    }
}

struct State {
    queue: FairQueue<ChannelId, (Message, Arc<Channel>)>,
    stopped: bool,
}

impl StatsSource for InboundMessageQueue {
    fn collect_stats(&self, result: &mut StatsCollection) {
        // Current queue depth for observability
        let guard = self.state.lock().unwrap();
        result.insert("message_processor_queue", "size", guard.queue.len());
        drop(guard);

        self.stats.collect_stats(result);
    }
}

#[derive(Default)]
struct MsgQueueStats {
    processed: AtomicUsize,
    processed_type: [AtomicUsize; MessageType::max_id() + 1],
    overfill: AtomicUsize,
    overfill_type: [AtomicUsize; MessageType::max_id() + 1],
}

impl StatsSource for MsgQueueStats {
    fn collect_stats(&self, result: &mut StatsCollection) {
        result.insert(
            "message_processor",
            "process",
            self.processed.load(Ordering::Relaxed),
        );
        for i in MessageType::iter() {
            result.insert(
                "message_processor_type",
                i.as_str(),
                self.processed_type[i as usize].load(Ordering::Relaxed),
            );
        }
        result.insert(
            "message_processor",
            "overfill",
            self.overfill.load(Ordering::Relaxed),
        );
        for i in MessageType::iter() {
            result.insert(
                "message_processor_overfill",
                i.as_str(),
                self.overfill_type[i as usize].load(Ordering::Relaxed),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsnano_messages::Message;

    #[test]
    fn put_and_get_one_message() {
        let manager = InboundMessageQueue::new(1);
        assert_eq!(manager.size(), 0);
        manager.put(Message::BulkPush, Arc::new(Channel::new_test_instance()));
        assert_eq!(manager.size(), 1);
        assert_eq!(manager.next_batch(1000).len(), 1);
        assert_eq!(manager.size(), 0);
    }
}
