use autoresolve_macros::{base, composite, resolvable};

#[derive(Clone)]
pub struct Scheduler;

impl Scheduler {
    fn number(&self) -> i32 {
        42
    }
}

#[derive(Clone)]
pub struct Clock;

impl Clock {
    fn number(&self) -> i32 {
        42
    }
}

#[composite(builtins)]
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
struct Client {
    validator: Validator,
    clock: Clock,
}

#[resolvable]
impl Client {
    fn new(validator: &Validator, clock: &Clock) -> Self {
        Self {
            validator: validator.clone(),
            clock: clock.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.validator.number() + self.clock.number()
    }
}

#[derive(Clone)]
struct Config {
    clock: Clock,
    scheduler: Scheduler,
}

#[resolvable]
impl Config {
    fn new(clock: &Clock, scheduler: &Scheduler) -> Self {
        Self {
            clock: clock.clone(),
            scheduler: scheduler.clone(),
        }
    }

    fn number(&self) -> i32 {
        self.clock.number() * 2
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

#[base]
struct MyBase {
    #[spread]
    builtins: Builtins,
}

#[test]
fn test_autoresolve() {
    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    let mut resolver = autoresolve::Resolver::new(MyBase { builtins });

    let service = resolver.get::<MyService>();

    // Validator(42) + Clock(42) + Clock(42)*2 = 42 + 42 + 84 = 168
    assert_eq!(service.number(), 42 + 42 + 42 * 2);
}
