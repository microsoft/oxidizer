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
struct Client {
    validator: Validator,
    builtins: Builtins,
}

#[resolvable]
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

#[derive(Clone)]
struct Config {
    builtins: Builtins,
}

#[resolvable]
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

struct MyService {
    client: Client,
    config: Config,
}

#[resolvable]
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

#[test]
fn test_autoresolve() {
    let builtins = Builtins;

    let mut resolver = autoresolve::resolver!(Base, builtins: Builtins);

    let service = resolver.get::<MyService>();

    assert_eq!(service.number(), 42 + 42 + 42 + 42);
}
