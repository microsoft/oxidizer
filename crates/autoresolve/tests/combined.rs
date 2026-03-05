#![allow(dead_code)] // Test structs exist to exercise the DI graph, not all fields are read.

use autoresolve_macros::{composite, resolvable};

#[derive(Clone)]
struct AppConfig;

impl AppConfig {
    fn number(&self) -> i32 {
        42
    }
}

#[derive(Clone)]
struct Logger;

#[derive(Clone)]
#[composite]
struct Builtins {
    app_config: AppConfig,
    logger: Logger,
}

#[derive(Clone)]
struct Validator {
    app_config: AppConfig,
}

#[resolvable]
impl Validator {
    fn new(app_config: &AppConfig) -> Self {
        Self {
            app_config: app_config.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.app_config.number()
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
    app_config: AppConfig,
    telemetry: Telemetry,
}

#[resolvable]
impl Client {
    fn new(validator: &Validator, app_config: &AppConfig, telemetry: &Telemetry) -> Self {
        Self {
            validator: validator.clone(),
            app_config: app_config.clone(),
            telemetry: telemetry.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.validator.number() + self.app_config.number()
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
    logger: Logger,
}

#[resolvable]
impl OutboundClient {
    fn new(correlation_vector: &CorrelationVector, client: &Client, logger: &Logger) -> Self {
        Self {
            correlation_vector: correlation_vector.clone(),
            client: client.clone(),
            logger: logger.clone(),
        }
    }
}

#[test]
fn test_combined() {
    let builtins = Builtins {
        app_config: AppConfig,
        logger: Logger,
    };
    let telemetry = Telemetry;
    let request = Request;

    let mut resolver = autoresolve::resolver!(Base,
        ..builtins: Builtins,
        telemetry: Telemetry,
        request: Request,
    );

    let outbound = resolver.get::<OutboundClient>();
    // Verify the object was constructed — Client depends on Validator + AppConfig + Telemetry,
    // OutboundClient depends on CorrelationVector + Client + Logger.
    assert_eq!(outbound.client.number(), 42 + 42);
}
