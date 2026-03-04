use autoresolve::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode, ResolveFrom, Resolver};

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

impl ResolveFrom<Builtins> for Validator {
    type Inputs = ResolutionDepsNode<Builtins, ResolutionDepsEnd>;

    fn new(input: <Self::Inputs as ResolutionDeps<Builtins>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(builtins, _) = input;
        Self::new(builtins)
    }
}

#[derive(Clone)]
struct Telemetry;

#[derive(Clone)]
struct SdkProvider {
    telemetry: Telemetry,
}

impl SdkProvider {
    fn new(telemetry: &Telemetry) -> Self {
        Self {
            telemetry: telemetry.clone(),
        }
    }
}

impl ResolveFrom<Telemetry> for SdkProvider {
    type Inputs = ResolutionDepsNode<Telemetry, ResolutionDepsEnd>;

    fn new(input: <Self::Inputs as ResolutionDeps<Telemetry>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(telemetry, _) = input;
        Self::new(telemetry)
    }
}

#[derive(Clone)]
struct Client {
    validator: Validator,
    builtins: Builtins,
    telemetry: Telemetry,
}

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

impl CorrelationVector {
    fn new(request: &Request) -> Self {
        Self { request: request.clone() }
    }
}

impl ResolveFrom<Request> for CorrelationVector {
    type Inputs = ResolutionDepsNode<Request, ResolutionDepsEnd>;

    fn new(input: <Self::Inputs as ResolutionDeps<Request>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(request, _) = input;
        Self::new(request)
    }
}

struct OutboundClient {
    correlation_vector: CorrelationVector,
    client: Client,
    builtins: Builtins,
}

impl OutboundClient {
    fn new(correlation_vector: &CorrelationVector, client: &Client, builtins: &Builtins) -> Self {
        Self {
            correlation_vector: correlation_vector.clone(),
            client: client.clone(),
            builtins: builtins.clone(),
        }
    }
}
