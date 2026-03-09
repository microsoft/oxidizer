use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// Request-level root. The counter is used by [`super::correlation_vector::CorrelationVector`]
/// to stamp each instance.
#[derive(Clone)]
pub struct Request {
    pub(crate) counter: Arc<AtomicUsize>,
}
