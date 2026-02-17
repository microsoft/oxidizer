use autoresolve::{
    OtherResolver, ResolutionAlternativesEnd, ResolutionAlternativesNode, ResolutionBaseEnd, ResolutionBaseListNode, ResolutionDeps,
    ResolutionDepsEnd, ResolutionDepsNode, Resolvable, ResolveFrom, Resolver, new_other_resolver,
};

#[derive(Clone)]
struct Builtins;

impl Builtins {
    fn number(&self) -> i32 {
        42
    }
}

#[derive(Clone)]
struct TokioRuntime;

impl AsRef<TokioRuntime> for TokioRuntime {
    fn as_ref(&self) -> &TokioRuntime {
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

    fn new_tokio(tokio: &TokioRuntime) -> Self {
        Self { builtins: Builtins }
    }

    fn number(&self) -> i32 {
        self.builtins.number()
    }
}

impl Resolvable for Validator {
    type Alternatives = ResolutionAlternativesNode<
        ResolutionBaseListNode<Builtins, ResolutionBaseEnd>,
        ResolutionAlternativesNode<ResolutionBaseListNode<TokioRuntime, ResolutionBaseEnd>, ResolutionAlternativesEnd>,
    >;
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

impl Resolvable for Client {
    type Alternatives = ResolutionAlternativesNode<
        ResolutionBaseListNode<Validator, ResolutionBaseListNode<Builtins, ResolutionBaseEnd>>,
        ResolutionAlternativesEnd,
    >;
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

impl Resolvable for CorrelationVector {
    type Alternatives = ResolutionAlternativesNode<ResolutionBaseListNode<Request, ResolutionBaseEnd>, ResolutionAlternativesEnd>;
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
}

impl Resolvable for MyService {
    type Alternatives = ResolutionAlternativesNode<
        ResolutionBaseListNode<Client, ResolutionBaseListNode<CorrelationVector, ResolutionBaseEnd>>,
        ResolutionAlternativesEnd,
    >;
}

#[test]
fn test_other_autoresolve() {
    let resolver: OtherResolver<ResolutionBaseListNode<Request, ResolutionBaseListNode<Builtins, ResolutionBaseEnd>>> =
        new_other_resolver::<Builtins>().with_base::<Request>();

    let x = resolver.resolve::<Validator, _>();
    let x = resolver.resolve::<MyService, _>();
}
