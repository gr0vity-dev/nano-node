use std::{
    any::{Any, TypeId},
    sync::{Arc, RwLock},
};

use rsnano_nullable_clock::SteadyClock;
use rsnano_utils::{CancellationToken, ticker::Tickable};

use super::ActiveElectionsContainer;

/// Every 300ms tries to transitions election state and send votes + blocks
pub struct AecTicker {
    active_elections: Arc<RwLock<ActiveElectionsContainer>>,
    clock: Arc<SteadyClock>,
    plugins: Vec<Box<dyn AecTickerPlugin>>,
}

impl AecTicker {
    #[allow(dead_code)]
    pub fn new(
        active_elections: Arc<RwLock<ActiveElectionsContainer>>,
        clock: Arc<SteadyClock>,
    ) -> Self {
        Self {
            active_elections,
            clock,
            plugins: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn new_null() -> Self {
        Self {
            active_elections: Arc::new(RwLock::new(ActiveElectionsContainer::default())),
            clock: Arc::new(SteadyClock::new_null()),
            plugins: Vec::new(),
        }
    }

    pub fn add_plugin(&mut self, plugin: impl AecTickerPlugin + 'static) {
        self.plugins.push(Box::new(plugin));
    }

    #[allow(dead_code)]
    pub fn get_plugin<T>(&self) -> Option<&T>
    where
        T: AecTickerPlugin + 'static,
    {
        let Some(p) = self
            .plugins
            .iter()
            .find(|p| (***p).type_id() == TypeId::of::<T>())
        else {
            return None;
        };

        (*p).as_any().downcast_ref::<T>()
    }
}

impl Tickable for AecTicker {
    fn tick(&mut self, _cancel_token: &CancellationToken) {
        {
            let mut aec = self.active_elections.write().unwrap();
            aec.transition_time(self.clock.now());
        }

        for plugin in &mut self.plugins {
            plugin.run(&self.active_elections);
        }
    }
}

pub trait AecTickerPlugin: Send + 'static {
    fn run(&mut self, aec: &RwLock<ActiveElectionsContainer>);

    #[allow(dead_code)]
    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }

    #[allow(dead_code)]
    fn as_any(&self) -> &dyn Any;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::AecInsertRequest;
    use rsnano_nullable_clock::Timestamp;
    use rsnano_types::{BlockPriority, SavedBlock};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn call_plugins() {
        let mut ticker = AecTicker::new_null();
        let plugin = StubPlugin::default();
        let called = plugin.0.clone();
        ticker.add_plugin(plugin);

        let block = SavedBlock::new_test_instance_with_key(1);
        let now = Timestamp::new_test_instance();

        ticker
            .active_elections
            .write()
            .unwrap()
            .insert(
                AecInsertRequest::new_manual(block.clone(), BlockPriority::new_test_instance()),
                now,
            )
            .unwrap();

        ticker.tick(&CancellationToken::new_null());

        assert!(called.load(Ordering::Relaxed));
    }

    #[derive(Default)]
    struct StubPlugin(Arc<AtomicBool>);

    impl AecTickerPlugin for StubPlugin {
        fn run(&mut self, _aec: &RwLock<ActiveElectionsContainer>) {
            self.0.store(true, Ordering::Relaxed);
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }
}
