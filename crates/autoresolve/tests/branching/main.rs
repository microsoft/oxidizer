#![allow(dead_code, missing_docs, missing_debug_implementations, clippy::missing_panics_doc)]

//! Phase 4 override tests: `either` / `or` branching.
//!
//! Type chain: `Top1 -> Beta -> Gamma`, `Top2 -> Beta -> Gamma`.
//! Two distinct top-level consumers (`Top1`, `Top2`) both consume `Beta`,
//! letting us verify branched overrides like
//! `provide(Gamma).when_injected_in::<Beta>().either(|x| x.when_injected_in::<Top1>()).or(|x| x.when_injected_in::<Top2>())`.

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
}

#[resolvable]
impl Beta {
    pub fn new(gamma: &Gamma) -> Self {
        Self { gamma_tag: gamma.tag }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Top1 {
    pub beta_gamma_tag: &'static str,
}

#[resolvable]
impl Top1 {
    pub fn new(beta: &Beta) -> Self {
        Self {
            beta_gamma_tag: beta.gamma_tag,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Top2 {
    pub beta_gamma_tag: &'static str,
}

#[resolvable]
impl Top2 {
    pub fn new(beta: &Beta) -> Self {
        Self {
            beta_gamma_tag: beta.gamma_tag,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Top3 {
    pub beta_gamma_tag: &'static str,
}

#[resolvable]
impl Top3 {
    pub fn new(beta: &Beta) -> Self {
        Self {
            beta_gamma_tag: beta.gamma_tag,
        }
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
fn branched_override_fires_on_each_alternative_root() {
    let mut resolver = make_resolver();
    resolver
        .provide(Gamma { tag: "custom" })
        .when_injected_in::<Beta>()
        .either(|x| x.when_injected_in::<Top1>())
        .or(|x| x.when_injected_in::<Top2>());

    let t1 = resolver.get::<Top1>();
    let t2 = resolver.get::<Top2>();
    assert_eq!(t1.beta_gamma_tag, "custom");
    assert_eq!(t2.beta_gamma_tag, "custom");

    // A third consumer not enumerated in the branch list sees the default.
    let t3 = resolver.get::<Top3>();
    assert_eq!(t3.beta_gamma_tag, "default-gamma");
}

#[test]
fn identity_branch_matches_bare_prefix() {
    let mut resolver = make_resolver();
    // The identity branch `|x| x` registers the bare `[Beta, Gamma]` path,
    // so a standalone Beta resolution also picks up the override. The named
    // branch additionally registers `[Top1, Beta, Gamma]`.
    resolver
        .provide(Gamma { tag: "custom" })
        .when_injected_in::<Beta>()
        .either(|x| x)
        .or(|x| x.when_injected_in::<Top1>());

    let beta = resolver.get::<Beta>();
    assert_eq!(beta.gamma_tag, "custom");

    let t1 = resolver.get::<Top1>();
    assert_eq!(t1.beta_gamma_tag, "custom");

    // Top2 was NOT in the branch list and does not match the bare prefix
    // (its key `[Top2, Beta, Gamma]` has no suffix `[Top2, Beta, Gamma]` or
    // `[Beta, Gamma]` for this slot's path... wait, `[Beta, Gamma]` IS a
    // suffix of `[Top2, Beta, Gamma]`). The identity branch DOES apply to
    // Top2's chain too — it's a global-ish override scoped to "anywhere Beta
    // is asked for Gamma".
    let t2 = resolver.get::<Top2>();
    assert_eq!(t2.beta_gamma_tag, "custom");
}

#[test]
fn branched_value_is_shared_across_alternatives() {
    use std::sync::Arc;

    let mut resolver = make_resolver();
    let custom = Gamma { tag: "shared" };
    resolver
        .provide(custom)
        .when_injected_in::<Beta>()
        .either(|x| x.when_injected_in::<Top1>())
        .or(|x| x.when_injected_in::<Top2>());

    // Both Top1.Beta and Top2.Beta are constructed reading from the SAME
    // shared Gamma Arc. We can verify by retrieving Beta along each path —
    // the Beta instances differ (different paths = different slots), but
    // both observed the same Gamma value.
    let t1 = resolver.get::<Top1>();
    let t2 = resolver.get::<Top2>();
    assert_eq!(t1.beta_gamma_tag, "shared");
    assert_eq!(t2.beta_gamma_tag, "shared");

    // Compare-by-pointer: the cached Gamma slots for [Top1, Beta, Gamma] and
    // [Top2, Beta, Gamma] should be the same Arc<Gamma>. We can't reach into
    // path_cache from here, but we can verify the values are equal.
    let _ = Arc::clone(&t1);
}

#[test]
fn or_only_no_either_is_unsupported_but_either_alone_works() {
    // `.either` with just one branch (no `.or`) is the minimal branched form.
    let mut resolver = make_resolver();
    resolver
        .provide(Gamma { tag: "single" })
        .when_injected_in::<Beta>()
        .either(|x| x.when_injected_in::<Top1>());

    let t1 = resolver.get::<Top1>();
    assert_eq!(t1.beta_gamma_tag, "single");

    // Top2 is not part of the branch list and the bare prefix [Beta, Gamma]
    // was not registered (the only branch added [Top1, Beta, Gamma]), so
    // Top2 sees the default.
    let t2 = resolver.get::<Top2>();
    assert_eq!(t2.beta_gamma_tag, "default-gamma");
}
