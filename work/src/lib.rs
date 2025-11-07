mod cpu_work_generator;

#[cfg(feature = "opencl")]
mod gpu;
#[cfg(feature = "opencl")]
mod gpu_work_generator;

mod work_pool;
mod work_queue;
mod work_thread;
mod xorshift;

pub(crate) use cpu_work_generator::CpuWorkGenerator;
pub use rsnano_work_validation::{WORK_THRESHOLDS_STUB, WorkThresholds, dev_difficulty};
pub(crate) use work_pool::WorkGenerator;
pub use work_pool::{WorkPool, WorkPoolBuilder};
pub(crate) use work_queue::WorkQueueCoordinator;
pub use work_queue::WorkTicket;
pub(crate) use work_thread::WorkThread;
pub(crate) use xorshift::XorShift1024Star;

pub(crate) trait WorkRng {
    fn next_work(&mut self) -> u64;
}

#[derive(Clone, PartialEq, Debug)]
pub struct OpenClConfig {
    pub platform: usize,
    pub device: usize,
    pub threads: usize,
}

impl Default for OpenClConfig {
    fn default() -> Self {
        Self {
            platform: 0,
            device: 0,
            threads: 1024 * 1024,
        }
    }
}
