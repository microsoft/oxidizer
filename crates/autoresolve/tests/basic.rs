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

    fn new(input: <Self::Inputs as ResolutionDeps<Builtins>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(validator, ResolutionDepsNode(builtins, _)) = input;
        Self::new(validator, builtins)
    }
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

    fn new(input: <Self::Inputs as ResolutionDeps<Builtins>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(builtins, _) = input;
        Self::new(builtins)
    }
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

    fn new(input: <Self::Inputs as ResolutionDeps<Builtins>>::Resolved<'_>) -> Self {
        let ResolutionDepsNode(client, ResolutionDepsNode(config, _)) = input;
        Self::new(client, config)
    }
}

#[test]
fn test_autoresolve() {
    let mut resolver = Resolver::new(Builtins);

    let service = resolver.get::<MyService>();

    assert_eq!(service.number(), 42 + 42 + 42 + 42);
}
