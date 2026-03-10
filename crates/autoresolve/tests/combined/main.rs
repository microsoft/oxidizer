#![allow(dead_code)] // Test structs exist to exercise the DI graph, not all fields are read.

use autoresolve_macros::base;

// Each type lives in its own module (separate file) so the generated code must
// resolve paths across module boundaries — validating that `#[base]` and
// `#[resolvable]` produce correct impls even when not all types are in scope at
// the usage site.

mod clock;
mod http;
mod runtime;
use runtime::builtins;
mod scheduler;
mod telemetry;

mod client;
mod correlation_vector;
mod outbound_client;
mod sdk_provider;
mod validator;

#[base]
mod base {
    pub struct Base {
        #[spread]
        pub builtins: super::builtins::Builtins,
        pub telemetry: super::telemetry::Telemetry,
        pub request: super::http::request::Request,
    }
}

#[test]
fn test_combined() {
    use base::Base;
    use builtins::Builtins;
    use clock::Clock;
    use http::request::Request;
    use outbound_client::OutboundClient;
    use scheduler::Scheduler;
    use telemetry::Telemetry;

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
