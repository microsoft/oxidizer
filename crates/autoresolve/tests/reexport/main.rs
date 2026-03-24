use autoresolve::{Resolver, base};
pub use runtime::exports::Builtins;

mod runtime;

pub struct Clock;

pub struct MyDep;

#[base(scoped(Builtins), helper_module_exported_as = crate::my_base_helper)]
pub struct MyBase {
    pub dep: MyDep,
}

#[test]
fn test_reexport() {
    let resolver = Resolver::new(MyBase { dep: MyDep });
}