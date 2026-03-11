use autoresolve_macros::base;

// Each type lives in its own module (separate file) so the generated code must
// resolve paths across module boundaries — validating that `#[base]` and
// `#[resolvable]` produce correct impls even when not all types are in scope at
// the usage site.

mod runtime;

mod clock;
mod scheduler;

mod client;
mod config;
mod my_service;
mod validator;

#[base]
mod my_base {
    pub struct MyBase {
        #[spread]
        pub builtins: crate::runtime::builtins::Builtins,
    }
}

#[test]
fn test_autoresolve() {
    use clock::Clock;
    use my_base::MyBase;
    use my_service::MyService;
    use runtime::builtins::Builtins;
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
