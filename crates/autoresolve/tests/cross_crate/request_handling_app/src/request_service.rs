//! Per-request resolvable service.

use xc_http::client::HttpClient;
use xc_http::request::Request;

/// Cross-crate fixture service resolved at the request tier.
#[derive(Clone, Debug)]
pub struct RequestService {
    /// Indicates that the constructor ran (always `true`).
    pub built: bool,
}

#[autoresolve::resolvable]
impl RequestService {
    /// Constructs a [`RequestService`] from injected dependencies.
    pub fn new(_client: &HttpClient, _request: &Request) -> Self {
        Self { built: true }
    }
}
