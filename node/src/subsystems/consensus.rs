use super::lifecycle::Lifecycle;

/// Facade over consensus internals (active elections, vote processor, schedulers).
pub struct ConsensusSubsystem {
    _private: (),
}

impl ConsensusSubsystem {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Queue a local block for processing.
    pub fn enqueue_local_block(&self, _label: &str) {
        todo!("enqueue local block for processing")
    }

    /// Retrieve summary info about active elections.
    pub fn active_info(&self) -> String {
        todo!("return active elections info")
    }

    /// Generate and broadcast a vote for a block.
    pub fn generate_vote(&self, _block_hash: &str) {
        todo!("generate vote")
    }

    /// Return confirming set statistics used by telemetry/tests.
    pub fn confirming_set_info(&self) -> String {
        todo!("confirming set info")
    }
}

impl Lifecycle for ConsensusSubsystem {
    fn start(&mut self) {
        todo!("start consensus components")
    }

    fn stop(&mut self) {
        todo!("stop consensus components")
    }
}
