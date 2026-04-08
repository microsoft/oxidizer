#![allow(dead_code, missing_docs, missing_debug_implementations)] // Test helpers.

use autoresolve_macros::base;

// Each type lives in its own module (separate file) so the generated code must
// resolve paths across module boundaries — validating that `#[base]` and
// `#[resolvable]` produce correct impls even when not all types are in scope at
// the usage site.

mod clock;
mod http;
mod runtime;
mod scheduler;
mod telemetry;

mod client;
mod correlation_vector;
mod outbound_client;
mod sdk_provider;
mod validator;

use http::request::Request;
use runtime::Builtins;
use telemetry::Telemetry;

#[base(helper_module_exported_as = crate::base_helper)]
pub struct Base {
    #[spread]
    pub builtins: Builtins,
    pub telemetry: Telemetry,
    pub request: Request,
}

/// Resolves a deep dependency graph through a base with `#[spread]` and regular fields.
#[test]
fn test_combined() {
    use clock::Clock;
    use outbound_client::OutboundClient;
    use scheduler::Scheduler;

    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    let mut resolver = autoresolve::Resolver::new(Base {
        builtins,
        telemetry: Telemetry,
        request: Request,
    });

    let outbound = resolver.get::<OutboundClient>();
    // Verify the object was constructed — Client depends on Validator + Scheduler + Telemetry,
    // OutboundClient depends on CorrelationVector + Client + Clock.
    assert_eq!(outbound.client.number(), 42 + 42);
}
