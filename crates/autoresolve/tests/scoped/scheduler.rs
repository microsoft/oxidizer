use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// App-level root. The counter is used by [`super::validator::Validator`] to stamp each instance.
#[derive(Clone)]
pub struct Scheduler {
    pub(crate) counter: Arc<AtomicUsize>,
}
