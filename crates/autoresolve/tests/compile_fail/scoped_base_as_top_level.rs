use autoresolve_macros::base;

pub struct Clock;
pub struct MyDep;

#[base(helper_module_exported_as = crate::builtins_helper)]
pub struct Builtins {
    pub clock: Clock,
}

#[base(scoped(Builtins), helper_module_exported_as = crate::my_base_helper)]
pub struct MyBase {
    pub dep: MyDep,
}

fn main() {
    // MyBase is scoped under Builtins — it should only be constructible via
    // `resolver.scoped(MyBase { ... })`, not via `Resolver::new()`.
    let _resolver = autoresolve::Resolver::new(MyBase { dep: MyDep });
}
