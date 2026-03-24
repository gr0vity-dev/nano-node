use rsnano_utils::sync::backpressure_channel::{self, Sender};

use super::AecEvent;

#[derive(Clone)]
pub(crate) struct AecEventPublisher {
    sender: Sender<AecEvent>,
}

impl AecEventPublisher {
    pub fn new(sender: Sender<AecEvent>) -> Self {
        Self { sender }
    }

    pub fn null() -> Self {
        let (sender, _receiver) = backpressure_channel::channel(1);
        Self::new(sender)
    }

    pub fn publish(&self, event: AecEvent) {
        let _ = self.sender.send(event);
    }

    pub fn publish_all(&self, events: impl IntoIterator<Item = AecEvent>) {
        for event in events {
            self.publish(event);
        }
    }
}
