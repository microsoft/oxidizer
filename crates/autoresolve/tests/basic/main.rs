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

use runtime::Builtins;

#[base(helper_module_exported_as = crate::my_base_helper)]
pub struct MyBase {
    #[spread]
    pub builtins: Builtins,
}

/// Basic end-to-end: resolves a service from a base with spread builtins.
#[test]
fn test_autoresolve() {
    use clock::Clock;
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

pub mod aaa {
    pub mod bbb {
        #[macro_export]
        macro_rules! macro1 {
            () => {
                #[macro_export]
                macro_rules! macro3 {
                    () => {};
                }
            };
        }

        macro1!();

        pub use macro3 as macro3_reexport;
    }
}

crate::aaa::bbb::macro3_reexport!();
