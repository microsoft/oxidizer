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

/// A base defined in a private module and re-exported via `#[reexport_base]` can seed a resolver.
#[test]
fn test_reexport() {
    let parent = Resolver::new(Builtins { clock: Clock });
    let mut resolver = parent.scoped(MyBase { dep: MyDep });
    resolver.get::<UsesClock>();
}