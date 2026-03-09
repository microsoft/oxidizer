use std::sync::atomic::Ordering;

use autoresolve_macros::resolvable;

use super::request::Request;

/// Depends on Request (request-level). `instance` records which construction this was.
#[derive(Clone)]
pub struct CorrelationVector {
    pub(crate) instance: usize,
}

#[resolvable]
impl CorrelationVector {
    fn new(request: &Request) -> Self {
        Self {
            instance: request.counter.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }
}
