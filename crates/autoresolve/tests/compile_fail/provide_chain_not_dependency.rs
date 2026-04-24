//! `when_injected_in::<T>()` requires the current head to be a
//! `DependencyOf<T>`. Trying to extend the chain with a type that does not
//! consume the current head must be a compile error.

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

// `Unrelated` does not depend on `A`, so `A: DependencyOf<Unrelated>` is not
// implemented.
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
    // `A` is not a `DependencyOf<Unrelated>` (Unrelated::new takes &B, not &A).
    resolver.provide(A).when_injected_in::<Unrelated>();
}
