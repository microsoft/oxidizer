use autoresolve::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode, ResolveFrom};

#[derive(Clone)]
struct Builtins;

impl Builtins {
    fn number(&self) -> i32 {
        42
    }
}

impl AsRef<Builtins> for Builtins {
    fn as_ref(&self) -> &Builtins {
        self
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

impl<T: AsRef<Builtins> + Send + Sync + 'static> ResolveFrom<T> for Validator {
    type Inputs = ResolutionDepsNode<Builtins, Builtins, ResolutionDepsEnd>;

    fn new(input: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self {
        Self::new(input.0)
    }
}

#[derive(Clone)]
struct Client {
    validator: Validator,
    builtins: Builtins,
}

impl Client {
    fn new(validator: &Validator, builtins: &Builtins) -> Self {
        Self {
            validator: validator.clone(),
            builtins: builtins.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.validator.number() + self.builtins.number()
    }
}

impl<T: AsRef<Builtins> + Send + Sync + 'static> ResolveFrom<T> for Client {
    type Inputs = ResolutionDepsNode<Validator, Builtins, ResolutionDepsNode<Builtins, Builtins, ResolutionDepsEnd>>;

    fn new(input: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self {
        Self::new(input.0, input.1.0)
    }
}

#[derive(Clone)]
struct Request;

impl Request {
    fn req_number(&self) -> i32 {
        100
    }
}

impl AsRef<Request> for Request {
    fn as_ref(&self) -> &Request {
        self
    }
}

#[derive(Clone)]
struct CorrelationVector {
    request: Request,
}

impl CorrelationVector {
    fn new(request: &Request) -> Self {
        Self { request: request.clone() }
    }

    fn corr_number(&self) -> i32 {
        self.request.req_number() + 1
    }
}

impl<T: AsRef<Request> + Send + Sync + 'static> ResolveFrom<T> for CorrelationVector {
    type Inputs = ResolutionDepsNode<Request, Request, ResolutionDepsEnd>;

    fn new(input: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self {
        Self::new(input.0)
    }
}

struct BuiltinsAndRequest {
    builtins: Builtins,
    request: Request,
}
impl AsRef<Builtins> for BuiltinsAndRequest {
    fn as_ref(&self) -> &Builtins {
        &self.builtins
    }
}
impl AsRef<Request> for BuiltinsAndRequest {
    fn as_ref(&self) -> &Request {
        &self.request
    }
}

#[derive(Clone)]
struct MyService {
    client: Client,
    correlation_vector: CorrelationVector,
}

impl MyService {
    fn new(client: &Client, correlation_vector: &CorrelationVector) -> Self {
        Self {
            client: client.clone(),
            correlation_vector: correlation_vector.clone(),
        }
    }

    fn service_number(&self) -> i32 {
        self.client.number() + self.correlation_vector.corr_number()
    }
}

impl<T> ResolveFrom<T> for MyService
where
    T: AsRef<Builtins> + AsRef<Request> + Send + Sync + 'static,
{
    type Inputs = ResolutionDepsNode<Client, Builtins, ResolutionDepsNode<CorrelationVector, Request, ResolutionDepsEnd>>;

    fn new(input: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self {
        Self::new(input.0, input.1.0)
    }
}
