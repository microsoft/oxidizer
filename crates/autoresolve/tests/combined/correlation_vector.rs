use autoresolve_macros::resolvable;

use super::request::Request;

#[derive(Clone)]
pub struct CorrelationVector {
    request: Request,
}

#[resolvable]
impl CorrelationVector {
    fn new(request: &Request) -> Self {
        Self { request: request.clone() }
    }
}
