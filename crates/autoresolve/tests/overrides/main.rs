#![allow(dead_code, missing_docs, missing_debug_implementations, clippy::missing_panics_doc)]

//! Phase 3 override tests: `provide()` chains and longest-suffix lookup.
//!
//! Type chain used throughout: `Alpha -> Beta -> Gamma`.
//! - `Gamma` has no deps.
//! - `Beta::new(&Gamma)` consumes `Gamma`.
//! - `Alpha::new(&Beta)` consumes `Beta`.
//! - `Diamond` consumes both `Beta` directly and `Gamma` indirectly via a
//!   sibling type — used to verify path scoping.

use autoresolve::Resolver;
use autoresolve_macros::{base, resolvable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gamma {
    pub tag: &'static str,
}

#[resolvable]
impl Gamma {
    pub fn new() -> Self {
        Self { tag: "default-gamma" }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beta {
    pub gamma_tag: &'static str,
    pub tag: &'static str,
}

#[resolvable]
impl Beta {
    pub fn new(gamma: &Gamma) -> Self {
        Self {
            gamma_tag: gamma.tag,
            tag: "default-beta",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alpha {
    pub beta_gamma_tag: &'static str,
    pub beta_tag: &'static str,
}

#[resolvable]
impl Alpha {
    pub fn new(beta: &Beta) -> Self {
        Self {
            beta_gamma_tag: beta.gamma_tag,
            beta_tag: beta.tag,
        }
    }
}

/// Sibling type that also depends on `Gamma`, used to verify that a
/// `provide(Gamma).when_injected_in::<Beta>().when_injected_in::<Alpha>()`
/// override does NOT affect this sibling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub gamma_tag: &'static str,
}

#[resolvable]
impl Sibling {
    pub fn new(gamma: &Gamma) -> Self {
        Self { gamma_tag: gamma.tag }
    }
}

#[derive(Debug, Clone)]
pub struct Marker;

#[base(helper_module_exported_as = crate::base_helper)]
pub struct AppBase {
    pub _marker: Marker,
}

fn make_resolver() -> Resolver<AppBase> {
    Resolver::new(AppBase { _marker: Marker })
}

#[test]
fn unscoped_provide_replaces_classical_resolution() {
    let mut resolver = make_resolver();
    resolver.provide(Gamma { tag: "custom" });

    let alpha = resolver.get::<Alpha>();
    assert_eq!(alpha.beta_gamma_tag, "custom");

    let sibling = resolver.get::<Sibling>();
    assert_eq!(sibling.gamma_tag, "custom");
}

#[test]
fn direct_path_scoped_override_only_affects_target_consumer() {
    let mut resolver = make_resolver();
    resolver.provide(Gamma { tag: "custom" }).when_injected_in::<Beta>();

    let alpha = resolver.get::<Alpha>();
    // Alpha's Beta saw the overridden Gamma.
    assert_eq!(alpha.beta_gamma_tag, "custom");

    // A sibling that depends on Gamma directly (not via Beta) sees the default.
    let sibling = resolver.get::<Sibling>();
    assert_eq!(sibling.gamma_tag, "default-gamma");
}

#[test]
fn chained_override_only_fires_on_full_path() {
    let mut resolver = make_resolver();
    resolver
        .provide(Gamma { tag: "for-alpha-only" })
        .when_injected_in::<Beta>()
        .when_injected_in::<Alpha>();

    let alpha = resolver.get::<Alpha>();
    assert_eq!(alpha.beta_gamma_tag, "for-alpha-only");

    let sibling = resolver.get::<Sibling>();
    assert_eq!(sibling.gamma_tag, "default-gamma");

    // A standalone Beta (resolved at the root, not via Alpha) should also get
    // the default Gamma — its path is just [Beta], not [Alpha, Beta].
    let mut resolver2 = make_resolver();
    resolver2
        .provide(Gamma { tag: "for-alpha-only" })
        .when_injected_in::<Beta>()
        .when_injected_in::<Alpha>();
    let beta = resolver2.get::<Beta>();
    assert_eq!(beta.gamma_tag, "default-gamma");
}

#[test]
fn longest_suffix_wins_over_shorter() {
    let mut resolver = make_resolver();
    // Both registrations are eligible when resolving Gamma along [Alpha, Beta]:
    // - [Beta, Gamma]            → "shorter"
    // - [Alpha, Beta, Gamma]     → "longer"
    // The longer one wins.
    resolver.provide(Gamma { tag: "shorter" }).when_injected_in::<Beta>();
    resolver
        .provide(Gamma { tag: "longer" })
        .when_injected_in::<Beta>()
        .when_injected_in::<Alpha>();

    let alpha = resolver.get::<Alpha>();
    assert_eq!(alpha.beta_gamma_tag, "longer");

    // A standalone Beta still gets the [Beta, Gamma] override (shorter wins
    // because the longer one is not a suffix of [Beta, Gamma]).
    let mut resolver2 = make_resolver();
    resolver2.provide(Gamma { tag: "shorter" }).when_injected_in::<Beta>();
    resolver2
        .provide(Gamma { tag: "longer" })
        .when_injected_in::<Beta>()
        .when_injected_in::<Alpha>();
    let beta = resolver2.get::<Beta>();
    assert_eq!(beta.gamma_tag, "shorter");
}

#[test]
fn unscoped_provide_acts_as_global_default() {
    let mut resolver = make_resolver();
    resolver.provide(Gamma { tag: "global" });

    let beta = resolver.get::<Beta>();
    assert_eq!(beta.gamma_tag, "global");
    let sibling = resolver.get::<Sibling>();
    assert_eq!(sibling.gamma_tag, "global");
}
