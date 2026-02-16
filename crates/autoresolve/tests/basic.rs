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

impl ResolveFrom<Builtins> for Validator {
    type Inputs = ResolutionDepsNode<Builtins, ResolutionDepsEnd>;

    fn new_resolved_from(input: <Self::Inputs as ResolutionDeps<Builtins>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(builtins, _) = input;
        Self::new(builtins)
    }
}

impl ResolveFrom<TokioRuntime> for Validator {
    type Inputs = ResolutionDepsNode<TokioRuntime, ResolutionDepsEnd>;

    fn new_resolved_from(input: <Self::Inputs as ResolutionDeps<TokioRuntime>>::Resolved<'_>) -> Self {
        Self::new_tokio(input.0)
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

impl ResolveFrom<Builtins> for Client {
    type Inputs = ResolutionDepsNode<Validator, ResolutionDepsNode<Builtins, ResolutionDepsEnd>>;

    fn new_resolved_from(input: <Self::Inputs as ResolutionDeps<Builtins>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(validator, ResolutionDepsNode(builtins, _)) = input;
        Self::new(validator, builtins)
    }
}

impl Resolvable for Client {
    type Alternatives = ResolutionAlternativesNode<
        ResolutionBaseListNode<Validator, ResolutionBaseListNode<Builtins, ResolutionBaseEnd>>,
        ResolutionAlternativesEnd,
    >;
}

#[derive(Clone)]
struct Config {
    builtins: Builtins,
}

impl Config {
    fn new(builtins: &Builtins) -> Self {
        Self {
            builtins: builtins.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.builtins.number() * 2
    }
}

impl ResolveFrom<Builtins> for Config {
    type Inputs = ResolutionDepsNode<Builtins, ResolutionDepsEnd>;

    fn new_resolved_from(input: <Self::Inputs as ResolutionDeps<Builtins>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(builtins, _) = input;
        Self::new(builtins)
    }
}

impl Resolvable for Config {
    type Alternatives = ResolutionAlternativesNode<ResolutionBaseListNode<Builtins, ResolutionBaseEnd>, ResolutionAlternativesEnd>;
}

struct MyService {
    client: Client,
    config: Config,
}

impl MyService {
    fn new(client: &Client, config: &Config) -> Self {
        Self {
            client: client.clone(),
            config: config.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.client.number() + self.config.number()
    }
}

impl ResolveFrom<Builtins> for MyService {
    type Inputs = ResolutionDepsNode<Client, ResolutionDepsNode<Config, ResolutionDepsEnd>>;

    fn new_resolved_from(input: <Self::Inputs as ResolutionDeps<Builtins>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(client, ResolutionDepsNode(config, _)) = input;
        Self::new(client, config)
    }
}

impl Resolvable for MyService {
    type Alternatives = ResolutionAlternativesNode<
        ResolutionBaseListNode<Client, ResolutionBaseListNode<Config, ResolutionBaseEnd>>,
        ResolutionAlternativesEnd,
    >;
}

#[test]
fn test_autoresolve() {
    let mut resolver = Resolver::new(Builtins);

    let service = resolver.get::<MyService>();

    assert_eq!(service.number(), 42 + 42 + 42 + 42);
}

#[test]
fn test_other_autoresolve() {
    let resolver = new_other_resolver::<Builtins>();

    let x = resolver.resolve::<Validator, _>();
    let x = resolver.resolve::<MyService, _>();
}
