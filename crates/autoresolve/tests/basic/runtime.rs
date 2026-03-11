use autoresolve_macros::base;

use super::{clock, scheduler};

#[base]
pub mod builtins {
    #[derive(Clone)]
    pub struct Builtins {
        pub scheduler: super::scheduler::Scheduler,
        pub clock: super::clock::Clock,
    }

    pub use super::scheduler::Scheduler as Builtins_Part1;

    
}
