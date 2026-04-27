//! `when_injected_in::<T>()` inside an `either` / `or` branch must enforce
//! the same `DependencyOf` bound as the linear builder.

use autoresolve::Resolver;
use autoresolve_macros::{base, resolvable};

#[derive(Debug, Clone)]
pub struct A;

#[resolvable]
impl A {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone)]
pub struct B;

#[resolvable]
impl B {
    pub fn new() -> Self {
        Self
    }
}

// `Unrelated` does not depend on `A`.
#[derive(Debug, Clone)]
pub struct Unrelated;

#[resolvable]
impl Unrelated {
    pub fn new(_b: &B) -> Self {
        Self
    }
}

#[derive(Debug, Clone)]
pub struct Marker;

#[base(helper_module_exported_as = crate::helper)]
pub struct Base {
    pub _marker: Marker,
}

fn main() {
    let mut resolver = Resolver::new(Base { _marker: Marker });
    // `A` is not a `DependencyOf<Unrelated>` — must fail inside the branch.
    resolver.provide(A).either(|x| x.when_injected_in::<Unrelated>());
}
