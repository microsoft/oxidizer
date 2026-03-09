use autoresolve_macros::base;

pub mod clock;
pub mod scheduler;

#[base]
pub mod builtins {
    #[derive(Clone)]
    pub struct Builtins {
        pub scheduler: super::scheduler::Scheduler,
        pub clock: super::clock::Clock,
    }
}
