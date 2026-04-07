use autoresolve::{Resolver, base, resolvable};
pub use runtime::exports::Builtins;

mod runtime;

pub struct Clock;

pub struct MyDep;

pub struct UsesClock;

#[resolvable]
impl UsesClock {
    pub fn new(clock: &Clock) -> Self {
        Self
    }
}

#[base(scoped(Builtins), helper_module_exported_as = crate::my_base_helper)]
pub struct MyBase {
    pub dep: MyDep,
}

#[test]
fn test_reexport() {
    let mut resolver = Resolver::new(MyBase { dep: MyDep });
    resolver.get::<UsesClock>();
}