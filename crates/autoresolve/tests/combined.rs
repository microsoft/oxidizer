#![allow(dead_code)] // Test structs exist to exercise the DI graph, not all fields are read.

use autoresolve_macros::base;

// Each type lives in its own module so the generated code must resolve paths
// across module boundaries — validating that `#[base]` and `#[resolvable]`
// produce correct impls even when not all types are in scope at the usage site.

mod scheduler {
    #[derive(Clone)]
    pub struct Scheduler;

    impl Scheduler {
        pub(crate) fn number(&self) -> i32 {
            42
        }
    }
}

mod clock {
    #[derive(Clone)]
    pub struct Clock;
}

mod telemetry {
    #[derive(Clone)]
    pub struct Telemetry;
}

mod request {
    #[derive(Clone)]
    pub struct Request;
}

#[base]
mod builtins {
    #[derive(Clone)]
    pub struct Builtins {
        pub scheduler: super::scheduler::Scheduler,
        pub clock: super::clock::Clock,
    }
}

mod validator {
    use autoresolve_macros::resolvable;

    use super::scheduler::Scheduler;

    #[derive(Clone)]
    pub struct Validator {
        scheduler: Scheduler,
    }

    #[resolvable]
    impl Validator {
        fn new(scheduler: &Scheduler) -> Self {
            Self {
                scheduler: scheduler.clone(),
            }
        }

        pub(crate) fn number(&self) -> i32 {
            self.scheduler.number()
        }
    }
}

mod sdk_provider {
    use autoresolve_macros::resolvable;

    use super::telemetry::Telemetry;

    #[derive(Clone)]
    pub struct SdkProvider {
        telemetry: Telemetry,
    }

    #[resolvable]
    impl SdkProvider {
        fn new(telemetry: &Telemetry) -> Self {
            Self {
                telemetry: telemetry.clone(),
            }
        }
    }
}

mod client {
    use autoresolve_macros::resolvable;

    use super::scheduler::Scheduler;
    use super::telemetry::Telemetry;
    use super::validator::Validator;

    #[derive(Clone)]
    pub struct Client {
        validator: Validator,
        scheduler: Scheduler,
        telemetry: Telemetry,
    }

    #[resolvable]
    impl Client {
        fn new(validator: &Validator, scheduler: &Scheduler, telemetry: &Telemetry) -> Self {
            Self {
                validator: validator.clone(),
                scheduler: scheduler.clone(),
                telemetry: telemetry.clone(),
            }
        }

        pub(crate) fn number(&self) -> i32 {
            self.validator.number() + self.scheduler.number()
        }
    }
}

mod correlation_vector {
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
}

mod outbound_client {
    use autoresolve_macros::resolvable;

    use super::client::Client;
    use super::clock::Clock;
    use super::correlation_vector::CorrelationVector;

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
}

#[base]
mod base {
    pub struct Base {
        #[spread]
        pub builtins: super::builtins::Builtins,
        pub telemetry: super::telemetry::Telemetry,
        pub request: super::request::Request,
    }
}

#[test]
fn test_combined() {
    use base::Base;
    use builtins::Builtins;
    use clock::Clock;
    use outbound_client::OutboundClient;
    use request::Request;
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
