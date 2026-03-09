#![allow(dead_code)] // Test structs exist to exercise the DI graph, not all fields are read.

use autoresolve_macros::{base, resolvable};

#[derive(Clone)]
pub struct Scheduler;

impl Scheduler {
    fn number(&self) -> i32 {
        42
    }
}

#[derive(Clone)]
pub struct Clock;

#[base]
mod builtins {
    #[derive(Clone)]
    pub struct Builtins {
        pub scheduler: super::Scheduler,
        pub clock: super::Clock,
    }
}

use builtins::Builtins;

#[derive(Clone)]
struct Validator {
    scheduler: Scheduler,
}

#[resolvable]
impl Validator {
    fn new(scheduler: &Scheduler) -> Self {
        Self {
            scheduler: scheduler.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.scheduler.number()
    }
}

#[derive(Clone)]
pub struct Telemetry;

#[derive(Clone)]
struct SdkProvider {
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

#[derive(Clone)]
struct Client {
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

    fn number(&self) -> i32 {
        self.validator.number() + self.scheduler.number()
    }
}

#[derive(Clone)]
pub struct Request;

#[derive(Clone)]
struct CorrelationVector {
    request: Request,
}

#[resolvable]
impl CorrelationVector {
    fn new(request: &Request) -> Self {
        Self { request: request.clone() }
    }
}

struct OutboundClient {
    correlation_vector: CorrelationVector,
    client: Client,
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

#[base]
mod base {
    pub struct Base {
        #[spread]
        pub builtins: super::Builtins,
        pub telemetry: super::Telemetry,
        pub request: super::Request,
    }
}

use base::Base;

#[test]
fn test_combined() {
    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };
    let telemetry = Telemetry;
    let request = Request;

    let mut resolver = autoresolve::Resolver::new(Base {
        builtins,
        telemetry,
        request,
    });

    let outbound = resolver.get::<OutboundClient>();
    // Verify the object was constructed — Client depends on Validator + Scheduler + Telemetry,
    // OutboundClient depends on CorrelationVector + Client + Clock.
    assert_eq!(outbound.client.number(), 42 + 42);
}
