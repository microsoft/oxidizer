/// Marker trait declaring that `Self` is a direct constructor dependency of `Target`.
///
/// Emitted automatically by the [`#[resolvable]`](crate::resolvable) proc macro
/// for each `&Dep` parameter of `Target::new`. For an `impl` block on
/// `Target` whose `fn new` accepts `&Dep`, the macro generates:
///
/// ```ignore
/// impl ::autoresolve::DependencyOf<Target> for Dep {}
/// ```
///
/// The trait is consumed by the override builder API to validate, at compile
/// time, that each link of an injection path corresponds to a declared
/// dependency relationship.
pub trait DependencyOf<Target: ?Sized> {}
