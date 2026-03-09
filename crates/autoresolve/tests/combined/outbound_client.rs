use autoresolve_macros::resolvable;

use super::client::Client;
use super::correlation_vector::CorrelationVector;
use super::runtime::clock::Clock;

pub struct OutboundClient {
    correlation_vector: CorrelationVector,
    pub(crate) client: Client,
    clock: Clock,
}

#[resolvable]
impl OutboundClient {
    fn new(correlation_vector: &CorrelationVector, client: &Client, clock: &Clock) -> Self {
        Self {
            correlation_vector: correlation_vector.clone(),
            client: client.clone(),
            clock: clock.clone(),
        }
    }
}
