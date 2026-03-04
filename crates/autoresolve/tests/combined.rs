#![allow(dead_code)] // Test structs exist to exercise the DI graph, not all fields are read.

use autoresolve_macros::resolvable;

#[derive(Clone)]
struct Builtins;

impl Builtins {
    fn number(&self) -> i32 {
        42
    }
}

#[derive(Clone)]
struct Validator {
    builtins: Builtins,
}

#[resolvable]
impl Validator {
    fn new(builtins: &Builtins) -> Self {
        Self {
            builtins: builtins.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.builtins.number()
    }
}

#[derive(Clone)]
struct Telemetry;

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
    builtins: Builtins,
    telemetry: Telemetry,
}

#[resolvable]
impl Client {
    fn new(validator: &Validator, builtins: &Builtins, telemetry: &Telemetry) -> Self {
        Self {
            validator: validator.clone(),
            builtins: builtins.clone(),
            telemetry: telemetry.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.validator.number() + self.builtins.number()
    }
}

#[derive(Clone)]
struct Request;

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
    builtins: Builtins,
}

#[resolvable]
impl OutboundClient {
    fn new(correlation_vector: &CorrelationVector, client: &Client, builtins: &Builtins) -> Self {
        Self {
            correlation_vector: correlation_vector.clone(),
            client: client.clone(),
            builtins: builtins.clone(),
        }
    }
}

#[test]
fn test_combined() {
    let builtins = Builtins;
    let telemetry = Telemetry;
    let request = Request;

    let mut resolver = autoresolve::resolver!(
        builtins: Builtins,
        telemetry: Telemetry,
        request: Request,
    );

    let outbound = resolver.get::<OutboundClient>();
    // Verify the object was constructed — Client depends on Validator + Builtins + Telemetry,
    // OutboundClient depends on CorrelationVector + Client + Builtins.
    assert_eq!(outbound.client.number(), 42 + 42);
}
