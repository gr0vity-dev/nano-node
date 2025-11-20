/// Lifecycle contract shared by all subsystem façade wrappers.
///
/// Implementors should start and stop their owned threads/resources.
pub trait Lifecycle {
    fn start(&mut self);
    fn stop(&mut self);
}
