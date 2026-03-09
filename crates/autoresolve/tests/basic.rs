use autoresolve_macros::base;

// Each type lives in its own module so the generated code must resolve paths
// across module boundaries — validating that `#[base]` and `#[resolvable]`
// produce correct impls even when not all types are in scope at the usage site.

mod scheduler {
    #[derive(Clone)]
    pub struct Scheduler;

    impl Scheduler {
        pub(crate) fn number(&self) -> i32 {
            42
        }
    }
}

mod clock {
    #[derive(Clone)]
    pub struct Clock;

    impl Clock {
        pub(crate) fn number(&self) -> i32 {
            42
        }
    }
}

#[base]
mod builtins {
    #[derive(Clone)]
    pub struct Builtins {
        pub scheduler: super::scheduler::Scheduler,
        pub clock: super::clock::Clock,
    }
}

mod validator {
    use autoresolve_macros::resolvable;

    use super::scheduler::Scheduler;

    #[derive(Clone)]
    pub struct Validator {
        scheduler: Scheduler,
    }

    #[resolvable]
    impl Validator {
        fn new(scheduler: &Scheduler) -> Self {
            Self {
                scheduler: scheduler.clone(),
            }
        }

        pub(crate) fn number(&self) -> i32 {
            self.scheduler.number()
        }
    }
}

mod client {
    use autoresolve_macros::resolvable;

    use super::clock::Clock;
    use super::validator::Validator;

    #[derive(Clone)]
    pub struct Client {
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

        pub(crate) fn number(&self) -> i32 {
            self.validator.number() + self.clock.number()
        }
    }
}

mod config {
    use autoresolve_macros::resolvable;

    use super::clock::Clock;
    use super::scheduler::Scheduler;

    #[derive(Clone)]
    pub struct Config {
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

        pub(crate) fn number(&self) -> i32 {
            self.clock.number() * 2
        }
    }
}

mod my_service {
    use autoresolve_macros::resolvable;

    use super::client::Client;
    use super::config::Config;

    pub struct MyService {
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

        pub(crate) fn number(&self) -> i32 {
            self.client.number() + self.config.number()
        }
    }
}

#[base]
mod my_base {
    pub struct MyBase {
        #[spread]
        pub builtins: super::builtins::Builtins,
    }
}

#[test]
fn test_autoresolve() {
    use builtins::Builtins;
    use clock::Clock;
    use my_base::MyBase;
    use my_service::MyService;
    use scheduler::Scheduler;

    let builtins = Builtins {
        scheduler: Scheduler,
        clock: Clock,
    };

    let mut resolver = autoresolve::Resolver::new(MyBase { builtins });

    let service = resolver.get::<MyService>();

    // Validator(42) + Clock(42) + Clock(42)*2 = 42 + 42 + 84 = 168
    assert_eq!(service.number(), 42 + 42 + 42 * 2);
}
