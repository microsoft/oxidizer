//! Compile-time dependency injection with minimal boilerplate.
//!
//! # What
//!
//! `autoresolve` is a dependency injection library focused on minimizing
//! boilerplate in common use cases.
//!
//! ```
//! # mod my_runtime {
//! # }
//! 
//! use my_runtime::{Scheduler, Clock, Builtins};
//! use autoresolve::{Resolver, resolvable};
//!
//! pub struct Validator { /* ... */ }
//!
//! #[resolvable]
//! impl Validator {
//!     fn new(scheduler: &Scheduler) -> Self { /* ... */ }
//! }
//!
//! pub struct Client { /* ... */ }
//!
//! #[resolvable]
//! impl Client {
//!     fn new(validator: &Validator, scheduler: &Scheduler, clock: &Clock) -> Self {
//!         /* ... */
//!     }
//! }
//!
//! fn main(builtins: Builtins) {
//!     let mut resolver = Resolver::new(builtins);
//!     let client = resolver.get::<Client>();
//! }
//! ```
//!
//! # Why
//!
//! - **Less boilerplate.** Services list the things they need by reference and
//!   the framework figures out how everything fits together.
//! - **Optimized for the common case.** Most DI frameworks center on
//!   flexibility — separating interfaces from implementations so any
//!   implementation can be swapped in. In practice almost every service has
//!   one production implementation plus a few test seams. `autoresolve`
//!   optimizes for that: the default wiring needs no annotation beyond
//!   declaring dependencies, while overrides for tests and special cases are
//!   explicit (see [Override functionality](#override-functionality)).
//! - **Compile-time safety.** Missing dependencies and dependency cycles are
//!   compiler errors, not runtime panics. You can only call `get::<T>()` for
//!   a type whose full transitive dependency graph is satisfiable from the
//!   resolver's bases.
//!
//! # Defining services
//!
//! Apply [`#[resolvable]`](resolvable) to an `impl` block whose `fn new(...)`
//! method takes shared references to the service's dependencies and returns
//! `Self`:
//!
//! ```
//! # mod my_runtime {
//! # }
//!
//! use my_runtime::{Scheduler, Clock}; 
//! use autoresolve::resolvable;
//!
//! pub struct Client {
//!     validator: Validator,
//!     clock: Clock,
//! }
//!
//! #[resolvable]
//! impl Client {
//!     fn new(validator: &Validator, clock: &Clock) -> Self {
//!         Self { validator: validator.clone(), clock: clock.clone() }
//!     }
//!
//!     // Other inherent methods are preserved unchanged.
//!     pub fn number(&self) -> i32 { /* ... */ }
//! }
//! ```
//!
//! The macro generates a [`ResolveFrom`] impl based on the signature of `new`.
//! See the [`#[resolvable]`](resolvable) macro documentation for details. You
//! can also implement `ResolveFrom` manually, but pay attention to its
//! contract to avoid runtime failures.
//!
//! # Using in applications
//!
//! Construct a [`Resolver`] from a base value (usually provided by the framework
//! you're using), then call [`get::<T>()`](Resolver::get) for any service:
//!
//! ```
//! use autoresolve::Resolver;
//!
//! fn main(builtins: &Builtins) {
//!     let mut resolver = Resolver::new(builtins);
//!     let client = resolver.get::<Client>();
//! }
//! ```
//!
//! Values are cached by type (with some caveats around overrides), so the
//! same service is only constructed once per resolver and shared by all
//! consumers that depend on it.
//!
//! # Custom Bases
//!
//! A *base* is a struct annotated with [`#[base]`](base) whose fields are
//! pre-inserted into the resolver as root values. Any field type listed in a
//! base is available as a dependency to `#[resolvable]` services without
//! itself needing a `#[resolvable]` impl.
//!
//! ```ignore
//! use autoresolve::{Resolver, base};
//!
//! pub struct Scheduler;
//! pub struct Clock;
//!
//! #[base(helper_module_exported_as = crate::app_base_helper)]
//! pub struct AppBase {
//!     pub scheduler: Scheduler,
//!     pub clock: Clock,
//! }
//!
//! let mut resolver = Resolver::new(AppBase {
//!     scheduler: Scheduler,
//!     clock: Clock,
//! });
//! ```
//! 
//! Note that the macro generates a helper module based on the provided
//! `helper_module_exported_as`. See [`#[reexport_base]`](reexport_base) 
//! for re-exporting a base defined in another crate/module so that
//! downstream crates can refer to it as if it were defined locally.
//! 
//! Most applications will never write a base of their own — frameworks and
//! library entrypoints typically define the base. Define a custom one only
//! when you have foundational services that aren't reachable from a
//! framework-supplied base. Often a leaf service (no dependencies) declared
//! with `#[resolvable]` is sufficient and avoids the need for a custom base.
//!
//! ## Spread
//!
//! Marking a base field with `#[spread]` reuses an existing base type as a
//! group of root values rather than a single value. The fields of the
//! spread-in base are inserted individually, exactly as if they had been
//! listed inline. This lets a runtime crate publish a `Builtins` base that
//! every framework or application can pull in:
//!
//! ```ignore
//! // In `my_runtime`:
//! #[autoresolve::base(helper_module_exported_as = crate::builtins_helper)]
//! pub struct Builtins {
//!     pub scheduler: Scheduler,
//!     pub clock: Clock,
//! }
//!
//! // In a downstream crate:
//! #[autoresolve::base(helper_module_exported_as = crate::app_base_helper)]
//! pub struct AppBase {
//!     #[spread]
//!     pub builtins: my_runtime::Builtins,
//!     pub app_context: AppContext,
//! }
//! ```
//!
//! After spreading, both `Scheduler` and `Clock` (the fields of `Builtins`)
//! are available as root values, alongside `AppContext`.
//!
//! # Scoped Bases
//!
//! Consider an HTTP server: long-lived services like the scheduler, HTTP
//! clients, and configuration live for the lifetime of the process, but each
//! incoming request also brings request-scoped values — the parsed
//! `Request`, per-request state — that some services
//! need to depend on. A scoped base lets a per-request resolver inherit
//! everything from the application resolver while adding those request-tier
//! root values:
//!
//! ```ignore
//! use autoresolve::{Resolver, base, resolvable};
//!
//! #[base(scoped(AppBase), helper_module_exported_as = crate::request_base_helper)]
//! pub struct RequestBase {
//!     pub request: Request,
//! }
//!
//! pub struct RequestHandler { /* ... */ }
//!
//! #[resolvable]
//! impl RequestHandler {
//!     // Mixes an app-tier dependency (Client) with a request-tier one (Request).
//!     fn new(client: &Client, request: &Request) -> Self { /* ... */ }
//! }
//!
//! let app: Resolver<AppBase> = Resolver::new(AppBase { /* ... */ });
//!
//! // For each incoming request:
//! let mut req: Resolver<RequestBase> = app.scoped(RequestBase { request });
//! let handler = req.get::<RequestHandler>();
//! ```
//!
//! In short: declare the scoped base with `#[base(scoped(ParentBase))]` and
//! create a child resolver from a parent with [`Resolver::scoped()`].
//! Scoped resolvers can be nested arbitrarily deep
//! (`task = req.scoped(TaskBase { ... })`).
//!
//! Caching follows a "promote to the shallowest possible ancestor" rule:
//! when a service is constructed in a child, if all of its dependencies are
//! reachable from an ancestor it is cached *in that ancestor* instead of in
//! the child. Sibling children of the same parent therefore share the
//! constructed instance. A service that depends on a request-scoped value
//! stays in the request-tier cache and is dropped when the request resolver
//! is dropped.
//!
//! # Override functionality
//!
//! [`Resolver::provide`] registers a value to be returned in place of the
//! default-constructed one when a service of that type is requested. It
//! solves two recurring needs:
//!
//! - **Integration testing.** Replace a deep dependency with a mock without
//!   threading a fake all the way down the dependency graph — give the
//!   resolver the mock and every consumer of that type sees it.
//! - **Customization.** A production deployment can override one piece of
//!   the default wiring (a tuned client, an alternative implementation of a
//!   policy) without forking the framework.
//!
//! ## Basic override (by type)
//!
//! Replace every resolution of `Gamma` with a specific value:
//!
//! ```ignore
//! resolver.provide(Gamma { tag: "custom" });
//! let alpha = resolver.get::<Alpha>();   // Alpha's transitive Gamma is "custom"
//! let sibling = resolver.get::<Sibling>(); // also "custom"
//! ```
//!
//! ## Path-scoped override (`when_injected_in`)
//!
//! Restrict the override to consumers reached via a specific path. The chain
//! reads root-first: "this `Gamma` only when `Beta` asks for it":
//!
//! ```ignore
//! resolver.provide(Gamma { tag: "custom" })
//!         .when_injected_in::<Beta>();
//!
//! // Beta (and anything resolved through Beta) sees the override:
//! let alpha = resolver.get::<Alpha>();    // Alpha -> Beta -> custom Gamma
//!
//! // A Sibling that depends on Gamma directly (not via Beta) sees the default:
//! let sibling = resolver.get::<Sibling>();
//! ```
//!
//! Chains can be extended further (`.when_injected_in::<Beta>()
//! .when_injected_in::<Alpha>()`). When several registrations could match a
//! given resolution, the longest matching suffix wins.
//!
//! ## Branching (`either` / `or`)
//!
//! When the same override should apply along several alternative consumer
//! paths, use the `either` / `or` branch builder to register them in one
//! statement and share the value:
//!
//! ```ignore
//! resolver.provide(Gamma { tag: "custom" })
//!         .when_injected_in::<Beta>()
//!         .either(|x| x.when_injected_in::<Top1>())
//!         .or(|x| x.when_injected_in::<Top2>());
//!
//! // Top1 -> Beta -> custom Gamma
//! // Top2 -> Beta -> custom Gamma
//! // Top3 -> Beta -> default Gamma  (Top3 was not enumerated)
//! ```
//!
//! All branches share the same `Arc<Gamma>` — the override value is
//! constructed once and reused regardless of which alternative path a
//! consumer is resolved through.
//!
//! # Missing features
//!
//! - **Async and `Result` constructors.** Today `fn new(...) -> Self` is
//!   synchronous and infallible. Async and fallible constructors are planned
//!   and should be reasonably straightforward to add without disturbing the
//!   existing API.
//! - **Multiple resolutions for the same type.** Each type currently
//!   resolves to a single value per cache slot. Registering several
//!   alternative producers for the same type and choosing between them at
//!   resolution time is not yet supported.
//! - Currently, the get method returns an Arc which is not pretty, needs
//!   to be changed to return impl AsRef, or if we can somehow make it work,
//!   a reference directly.
//! - Thread awareness
//! - Integration of ohno

mod base_type;
mod dependency_of;
mod path_cache;
mod path_stack;
mod provide;
mod provide_path;
mod resolve_deps;
mod resolve_from;
mod resolve_output;
mod resolver;
mod resolver_macro;

#[cfg(feature = "macros")]
pub use autoresolve_macros::base;
#[cfg(feature = "macros")]
pub use autoresolve_macros::reexport_base;
#[cfg(feature = "macros")]
pub use autoresolve_macros::resolvable;
pub use base_type::BaseType;
pub use dependency_of::DependencyOf;
pub use path_stack::PathStack;
pub use provide::{BranchBuilder, Branched, ProvideBuilder};
pub use provide_path::{Scoped, Unscoped};
pub use resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};
pub use resolve_from::ResolveFrom;
pub use resolve_output::ResolveOutput;
pub use resolver::Resolver;
