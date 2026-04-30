//! Compile-time dependency injection with minimal boilerplate.
//!
//! # What
//!
//! `autoresolve` is a dependency injection library focused on minimizing
//! boilerplate in common use cases.
//!
//! ```
//! # pub mod my_runtime {
//! #     pub struct Scheduler;
//! #     pub struct Clock;
//! #     #[autoresolve::base(helper_module_exported_as = crate::my_runtime::builtins_helper)]
//! #     pub struct Builtins {
//! #         pub scheduler: Scheduler,
//! #         pub clock: Clock,
//! #     }
//! # }
//! use autoresolve::{Resolver, base, resolvable};
//! use my_runtime::{Scheduler, Clock, Builtins};
//!
//! // Define a service
//! pub struct Validator;
//! #[resolvable]
//! impl Validator {
//!     // Declare a dependency
//!     fn new(_scheduler: &Scheduler) -> Self { Self }
//! }
//!
//! // Define another service
//! pub struct Client;
//! #[resolvable]
//! impl Client {
//!     // Declare several dependencies, including one on another service (Validator)
//!     fn new(_validator: &Validator, _scheduler: &Scheduler, _clock: &Clock) -> Self {
//!         Self
//!     }
//! }
//!
//! fn my_main(builtins: Builtins) {
//!     let mut resolver = Resolver::new(builtins);
//!     // Obtain a service with all dependencies resolved
//!     let _client = resolver.get::<Client>();
//! }
//! # fn main() {}
//! ```
//!
//! # Why
//!
//! - **Less boilerplate.** Services list the things they need by reference and
//!   the framework figures out how everything fits together.
//! - **Optimized for the common case.** Most DI frameworks center on
//!   flexibility — separating interfaces from implementations so any
//!   implementation can be swapped in. In practice almost every service has
//!   one production implementation plus a few test ones. `autoresolve`
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
//! # #[derive(Clone)] 
//! # pub struct Validator;
//! # #[derive(Clone)] 
//! # pub struct Clock;
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
//!     // Other methods don't impact resolution.
//!     pub fn number(&self) -> i32 { 0 }
//! }
//! ```
//!
//! The macro generates a [`ResolveFrom`] impl based on the signature of `new`.
//! See the [`#[resolvable]`](resolvable) macro documentation for details. You
//! can also implement `ResolveFrom` manually, but pay attention to its
//! contract to avoid runtime failures.
//!
//! # Pulling services together
//!
//! Construct a [`Resolver`] from a base value (usually provided by the framework
//! you're using - Builtins in the example below), then call
//! [`get::<T>()`](Resolver::get) for any service:
//! 
//! ```
//! # pub mod my_runtime {
//! #     pub struct Scheduler;
//! #     pub struct Clock;
//! #     #[autoresolve::base(helper_module_exported_as = crate::my_runtime::builtins_helper)]
//! #     pub struct Builtins {
//! #         pub scheduler: Scheduler,
//! #         pub clock: Clock,
//! #     }
//! # }
//! use autoresolve::{Resolver, base, resolvable};
//! use my_runtime::{Scheduler, Clock, Builtins};
//!
//! pub struct Validator;
//! #[resolvable]
//! impl Validator {
//!     fn new(_scheduler: &Scheduler) -> Self { Self }
//! }
//!
//! pub struct Client;
//! #[resolvable]
//! impl Client {
//!     fn new(_validator: &Validator, _scheduler: &Scheduler, _clock: &Clock) -> Self {
//!         Self
//!     }
//! }
//!
//! fn my_main(builtins: Builtins) {
//!     let mut resolver = Resolver::new(builtins);
//!     let _client = resolver.get::<Client>();
//!     let _validator = resolver.get::<Validator>(); // Same validator as client's, cached by type
//! }
//! # fn main() {}
//! ```
//!
//! In this case, both `Validator` and `Client` depend on contents of `Builtins`, so they can
//! be constructed from the resolver. Note that the dependencies don't need to be direct - as
//! long as there is a path from the base to the service through the dependency graph, it can be 
//! resolved. Services are cached by type (with some caveats around overrides, see below), so the
//! same service is only constructed once per resolver and shared by all
//! consumers that depend on it.
//! 
//! ## Compile-time safety
//!
//! Because the dependency graph is encoded in the type system, mistakes are
//! caught by the compiler rather than surfacing as runtime panics.
//!
//! **Missing dependency.** A service whose dependencies cannot be supplied
//! by the base fails to compile when you try to resolve it:
//!
//! ```compile_fail
//! # pub mod my_runtime {
//! #     pub struct Scheduler;
//! #     #[autoresolve::base(helper_module_exported_as = crate::my_runtime::builtins_helper)]
//! #     pub struct Builtins { pub scheduler: Scheduler }
//! # }
//! use autoresolve::{Resolver, resolvable};
//! use my_runtime::{Scheduler, Builtins};
//!
//! pub struct Database; // Not in `Builtins`, not `#[resolvable]`.
//!
//! pub struct Repository;
//!
//! #[resolvable]
//! impl Repository {
//!     fn new(_db: &Database) -> Self { Self }
//! }
//!
//! fn my_main(builtins: Builtins) {
//!     let mut resolver = Resolver::new(builtins);
//!     // error[E0277]: the trait bound `Database: ResolveFrom<Builtins>` is not satisfied
//!     let _repo = resolver.get::<Repository>();
//! }
//! # fn main() {}
//! ```
//!
//! **Dependency cycle.** A cycle in the dependency graph also fails to
//! compile, surfacing as a trait-solver overflow rather than a stack
//! overflow at runtime:
//!
//! ```compile_fail
//! # pub mod my_runtime {
//! #     pub struct Scheduler;
//! #     #[autoresolve::base(helper_module_exported_as = crate::my_runtime::builtins_helper)]
//! #     pub struct Builtins { pub scheduler: Scheduler }
//! # }
//! use autoresolve::{Resolver, resolvable};
//! use my_runtime::{Scheduler, Builtins};
//!
//! pub struct A;
//! pub struct B;
//!
//! #[resolvable]
//! impl A {
//!     fn new(_b: &B) -> Self { Self }
//! }
//!
//! #[resolvable]
//! impl B {
//!     fn new(_a: &A) -> Self { Self }
//! }
//!
//! fn my_main(builtins: Builtins) {
//!     let mut resolver = Resolver::new(builtins);
//!     // error[E0275]: overflow evaluating the requirement `A: ResolveFrom<Builtins>`
//!     let _a = resolver.get::<A>();
//! }
//! # fn main() {}
//! ```
//!
//! # Custom Bases
//!
//! A *base* is a struct annotated with [`#[base]`](base) whose fields are
//! pre-inserted into the resolver as root values. Any field type listed in a
//! base is available as a dependency to `#[resolvable]` services without
//! itself needing a `#[resolvable]` impl.
//!
//! ```
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
//! fn main() {
//!     let _resolver = Resolver::new(AppBase {
//!         scheduler: Scheduler,
//!         clock: Clock,
//!     });
//! }
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
//! ```
//! use autoresolve::{Resolver, base};
//!
//! pub struct Scheduler;
//! pub struct Clock;
//! pub struct AppContext;
//!
//! // Imagine this lives in a runtime crate:
//! #[base(helper_module_exported_as = crate::builtins_helper)]
//! pub struct Builtins {
//!     pub scheduler: Scheduler,
//!     pub clock: Clock,
//! }
//!
//! // And this in a downstream crate:
//! #[base(helper_module_exported_as = crate::app_base_helper)]
//! pub struct AppBase {
//!     #[spread]
//!     pub builtins: Builtins,
//!     pub app_context: AppContext,
//! }
//!
//! fn main() {
//!     let _resolver = Resolver::new(AppBase {
//!         builtins: Builtins { scheduler: Scheduler, clock: Clock },
//!         app_context: AppContext,
//!     });
//! }
//! ```
//!
//! After spreading, both `Scheduler` and `Clock` (the fields of `Builtins`)
//! are available as root values, alongside `AppContext`.
//! 
//! Note that there are limitations around how the spread base is imported - see
//! the documentation for [`base`] for more information.
//!
//! # Scoped Bases
//!
//! Bases can be "scoped" - consider an HTTP server: long-lived services like the HTTP
//! clients and configuration live for the lifetime of the process, but each
//! incoming request also brings request-scoped values — the parsed
//! `Request`, per-request state — that some services
//! need to depend on. A scoped base lets a per-request resolver inherit
//! everything from the application resolver while adding those request-tier
//! root values:
//!
//! ```
//! # pub mod my_runtime {
//! #     pub struct Scheduler;
//! #     #[autoresolve::base(helper_module_exported_as = crate::my_runtime::builtins_helper)]
//! #     pub struct Builtins { pub scheduler: Scheduler }
//! # }
//! use my_runtime::{Scheduler};
//! use autoresolve::{Resolver, base, resolvable};
//!
//! pub struct Client;
//! #[resolvable]
//! impl Client {
//!     fn new(_scheduler: &Scheduler) -> Self { Self }
//! }
//!
//! #[derive(Clone)] pub struct Request;
//! #[base(scoped(my_runtime::Builtins), helper_module_exported_as = crate::request_base_helper)]
//! pub struct RequestBase {
//!     pub request: Request,
//! }
//!
//! pub struct RequestHandler;
//!
//! #[resolvable]
//! impl RequestHandler {
//!     // Mixes an app-tier dependency (Client) with a request-tier one (Request).
//!     fn new(_client: &Client, _request: &Request) -> Self { Self }
//! }
//!
//! fn my_main(builtins: my_runtime::Builtins) {
//!     let app = Resolver::new(builtins);
//!
//!     // For each incoming request:
//!     let mut req: Resolver<RequestBase> = app.scoped(RequestBase { request: Request });
//!     let _handler = req.get::<RequestHandler>();
//! }
//! # fn main() {}
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
//! Note that there are limitations around how the parent base is imported - see
//! the documentation for [`base`] for more information.
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
//! The remaining override examples share this `Alpha -> Beta -> Gamma` chain
//! plus a `Sibling` that depends on `Gamma` directly:
//!
//! ```
//! use autoresolve::{Resolver, base, resolvable};
//!
//! pub struct Gamma { pub tag: &'static str }
//! #[resolvable]
//! impl Gamma {
//!     fn new() -> Self { Self { tag: "default" } }
//! }
//!
//! pub struct Beta { pub gamma_tag: &'static str }
//! #[resolvable]
//! impl Beta {
//!     fn new(gamma: &Gamma) -> Self { Self { gamma_tag: gamma.tag } }
//! }
//!
//! pub struct Alpha { pub beta_gamma_tag: &'static str }
//! #[resolvable]
//! impl Alpha {
//!     fn new(beta: &Beta) -> Self { Self { beta_gamma_tag: beta.gamma_tag } }
//! }
//!
//! pub struct Sibling { pub gamma_tag: &'static str }
//! #[resolvable]
//! impl Sibling {
//!     fn new(gamma: &Gamma) -> Self { Self { gamma_tag: gamma.tag } }
//! }
//!
//! pub struct Top1 { pub beta_gamma_tag: &'static str }
//! #[resolvable]
//! impl Top1 {
//!     fn new(beta: &Beta) -> Self { Self { beta_gamma_tag: beta.gamma_tag } }
//! }
//!
//! pub struct Top2 { pub beta_gamma_tag: &'static str }
//! #[resolvable]
//! impl Top2 {
//!     fn new(beta: &Beta) -> Self { Self { beta_gamma_tag: beta.gamma_tag } }
//! }
//!
//! # fn main() {}
//! ```
//!
//! ## Basic override (by type)
//!
//! Replace every resolution of `Gamma` with a specific value:
//!
//! ```
//! # use autoresolve::{Resolver, base, resolvable};
//! # pub struct Gamma { pub tag: &'static str }
//! # #[resolvable] impl Gamma { fn new() -> Self { Self { tag: "default" } } }
//! # pub struct Beta { pub gamma_tag: &'static str }
//! # #[resolvable] impl Beta { fn new(gamma: &Gamma) -> Self { Self { gamma_tag: gamma.tag } } }
//! # pub struct Alpha { pub beta_gamma_tag: &'static str }
//! # #[resolvable] impl Alpha { fn new(beta: &Beta) -> Self { Self { beta_gamma_tag: beta.gamma_tag } } }
//! # pub struct Sibling { pub gamma_tag: &'static str }
//! # #[resolvable] impl Sibling { fn new(gamma: &Gamma) -> Self { Self { gamma_tag: gamma.tag } } }
//! # pub struct Marker;
//! # #[base(helper_module_exported_as = crate::app_base_helper)] pub struct Builtins { pub marker: Marker }
//! # fn main() {
//! # let builtins = Builtins { marker: Marker };
//! let mut resolver: Resolver<Builtins> = Resolver::new(builtins);
//!
//! resolver.provide(Gamma { tag: "custom" });
//! let alpha = resolver.get::<Alpha>();
//! assert_eq!(alpha.beta_gamma_tag, "custom");          // Alpha -> Beta -> custom Gamma
//! let sibling = resolver.get::<Sibling>();
//! assert_eq!(sibling.gamma_tag, "custom");             // Sibling also sees custom
//! # }
//! ```
//!
//! ## Path-scoped override (`when_injected_in`)
//!
//! Restrict the override to consumers reached via a specific path. The chain
//! reads root-first: "this `Gamma` only when `Beta` asks for it":
//!
//! ```
//! # use autoresolve::{Resolver, base, resolvable};
//! # pub struct Gamma { pub tag: &'static str }
//! # #[resolvable] impl Gamma { fn new() -> Self { Self { tag: "default" } } }
//! # pub struct Beta { pub gamma_tag: &'static str }
//! # #[resolvable] impl Beta { fn new(gamma: &Gamma) -> Self { Self { gamma_tag: gamma.tag } } }
//! # pub struct Alpha { pub beta_gamma_tag: &'static str }
//! # #[resolvable] impl Alpha { fn new(beta: &Beta) -> Self { Self { beta_gamma_tag: beta.gamma_tag } } }
//! # pub struct Sibling { pub gamma_tag: &'static str }
//! # #[resolvable] impl Sibling { fn new(gamma: &Gamma) -> Self { Self { gamma_tag: gamma.tag } } }
//! # pub struct Marker;
//! # #[base(helper_module_exported_as = crate::app_base_helper)] pub struct Builtins { pub marker: Marker }
//! # fn main() {
//! # let builtins = Builtins { marker: Marker };
//! let mut resolver: Resolver<Builtins> = Resolver::new(builtins);
//!
//! resolver.provide(Gamma { tag: "custom" })
//!         .when_injected_in::<Beta>();
//!
//! // Beta (and anything resolved through Beta) sees the override:
//! let alpha = resolver.get::<Alpha>();
//! assert_eq!(alpha.beta_gamma_tag, "custom");
//!
//! // A Sibling that depends on Gamma directly (not via Beta) sees the default:
//! let sibling = resolver.get::<Sibling>();
//! assert_eq!(sibling.gamma_tag, "default");
//! # }
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
//! ```
//! # use autoresolve::{Resolver, base, resolvable};
//! # pub struct Gamma { pub tag: &'static str }
//! # #[resolvable] impl Gamma { fn new() -> Self { Self { tag: "default" } } }
//! # pub struct Beta { pub gamma_tag: &'static str }
//! # #[resolvable] impl Beta { fn new(gamma: &Gamma) -> Self { Self { gamma_tag: gamma.tag } } }
//! # pub struct Top1 { pub beta_gamma_tag: &'static str }
//! # #[resolvable] impl Top1 { fn new(beta: &Beta) -> Self { Self { beta_gamma_tag: beta.gamma_tag } } }
//! # pub struct Top2 { pub beta_gamma_tag: &'static str }
//! # #[resolvable] impl Top2 { fn new(beta: &Beta) -> Self { Self { beta_gamma_tag: beta.gamma_tag } } }
//! # pub struct Marker;
//! # #[base(helper_module_exported_as = crate::app_base_helper)] pub struct Builtins { pub marker: Marker }
//! # fn main() {
//! # let builtins = Builtins { marker: Marker };
//! let mut resolver: Resolver<Builtins> = Resolver::new(builtins);
//!
//! resolver.provide(Gamma { tag: "custom" })
//!         .when_injected_in::<Beta>()
//!         .either(|x| x.when_injected_in::<Top1>())
//!         .or(|x| x.when_injected_in::<Top2>());
//!
//! assert_eq!(resolver.get::<Top1>().beta_gamma_tag, "custom");
//! assert_eq!(resolver.get::<Top2>().beta_gamma_tag, "custom");
//! # }
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
//! - Perf needs some work

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
#[doc(inline)]
/// Declares a struct as a resolver *base* — the set of root values from
/// which a [`Resolver`] (or a scoped child resolver) is built.
///
/// See [Custom Bases](crate#custom-bases) and [Scoped Bases](crate#scoped-bases)
/// in the crate-level docs for the conceptual overview. This page documents
/// the macro's surface and the code it expands to.
///
/// # Form
///
/// `#[base]` is applied to a struct with named fields:
///
/// ```
/// use autoresolve::base;
///
/// pub struct Scheduler;
/// pub struct Clock;
///
/// #[base(helper_module_exported_as = crate::app_base_helper)]
/// pub struct AppBase {
///     pub scheduler: Scheduler,
///     pub clock: Clock,
/// }
/// # fn main() {}
/// ```
///
/// The `helper_module_exported_as = crate::path::to::helper` argument is
/// **required**. It names the `crate::`-rooted absolute path at which the
/// macro will publish a hidden helper module containing the wiring used by
/// `#[spread]` and by [`#[reexport_base]`](reexport_base). The path must
/// match the module in which `#[base]` is invoked (e.g. for a base defined
/// at the crate root, pass `crate::app_base_helper`; inside `mod runtime`,
/// pass `crate::runtime::app_base_helper`).
///
/// Each field of the struct becomes a root value: it is pre-inserted into
/// the resolver and is available as a `&Field` dependency to any
/// `#[resolvable]` service. Fields do not need to be `#[resolvable]`
/// themselves.
///
/// ## `#[spread]` fields
///
/// A field annotated with `#[spread]` must itself be a `#[base]`-annotated
/// type. Its individual fields are spread into the surrounding base as if
/// they had been listed inline:
///
/// ```
/// use autoresolve::base;
///
/// pub struct Scheduler;
/// pub struct Clock;
/// pub struct AppContext;
///
/// // Imagine this lives in a runtime crate:
/// #[base(helper_module_exported_as = crate::builtins_helper)]
/// pub struct Builtins {
///     pub scheduler: Scheduler,
///     pub clock: Clock,
/// }
///
/// // And this in an application crate:
/// #[base(helper_module_exported_as = crate::app_base_helper)]
/// pub struct AppBase {
///     #[spread]
///     pub builtins: Builtins,
///     pub app_context: AppContext,
/// }
/// # fn main() {}
/// ```
///
/// After spreading, `Scheduler`, `Clock`, and `AppContext` are all root
/// values of `AppBase`. Spread chains transitively: a `#[spread]` field can
/// reference a base that itself has `#[spread]` fields.
///
/// ## Scoped bases
///
/// Adding `scoped(ParentBase)` declares the base as a scoped child of
/// `ParentBase`. Scoped bases seed *child* resolvers built with
/// [`Resolver::scoped`].
///
/// ```
/// use autoresolve::{Resolver, base};
///
/// pub struct Scheduler;
/// pub struct Request;
///
/// #[base(helper_module_exported_as = crate::app_base_helper)]
/// pub struct AppBase { pub scheduler: Scheduler }
///
/// #[base(scoped(AppBase), helper_module_exported_as = crate::request_base_helper)]
/// pub struct RequestBase {
///     pub request: Request,
/// }
///
/// fn main() {
///     let app: Resolver<AppBase> = Resolver::new(AppBase { scheduler: Scheduler });
///     let _req: Resolver<RequestBase> = app.scoped(RequestBase { request: Request });
/// }
/// ```
///
/// `scoped(...)` and `#[spread]` can be combined.
///
/// # Generated code
///
/// For each base, the macro emits:
///
/// - A hidden helper module at the path supplied by
///   `helper_module_exported_as`. The module re-exports each non-spread
///   field type under a mangled name (`Base_PartN_FieldType`) and defines a
///   declarative macro with `@impls`, `@insert`, and `@reexport` arms. The
///   `@impls` arm produces a stub `ResolveFrom<Base>` impl for every root
///   value (`fn new` is `unreachable!()` — root values are pre-inserted, not
///   constructed); the `@insert` arm destructures the base struct and
///   pushes each field into the resolver.
/// - An invocation of the `@impls` arm that materializes the
///   `ResolveFrom<Self>` impls.
/// - A [`BaseType`] impl whose `Parent` associated type is `()` for primary
///   bases and `ParentBase` for scoped ones, and whose `insert_into` calls
///   the `@insert` arm.
///
/// The mangling and helper-module redirection exist so that `#[spread]`
/// (and [`#[reexport_base]`](reexport_base)) can refer to a foreign base's
/// parts purely by macro path, without requiring the downstream crate to
/// import every individual field type.
pub use autoresolve_macros::base;
#[cfg(feature = "macros")]
#[doc(inline)]
/// Re-exports a base type defined in another module so that downstream code
/// can refer to it as if [`#[base]`](base) had been applied locally.
///
/// `#[base]` publishes its wiring under a hidden helper module rooted at a
/// `crate::`-rooted absolute path. When a base is `pub use`-d into another
/// module, that path no longer matches the new location — `#[spread]`
/// references and friendly re-exports would be looking for the helper at
/// the wrong place. `#[reexport_base]` solves this by emitting a fresh
/// helper module at the re-export site that forwards to the original.
///
/// # Form
///
/// Apply the attribute to a `pub type` alias whose right-hand side names
/// the original `#[base]` struct, supplying a `helper_module_exported_as`
/// path pointing at the *new* location:
///
/// ```
/// use autoresolve::{base, reexport_base};
///
/// // Original `#[base]` lives in a private module:
/// mod internal {
///     pub struct Scheduler;
///     pub struct Clock;
///
///     #[autoresolve::base(helper_module_exported_as = crate::internal::builtins_helper)]
///     pub struct Builtins {
///         pub scheduler: Scheduler,
///         pub clock: Clock,
///     }
/// }
///
/// pub mod public_api {
///     // Re-export it as `public_api::Builtins` so external crates use a clean path.
///     #[autoresolve::reexport_base(helper_module_exported_as = crate::public_api::builtins_helper)]
///     pub type Builtins = super::internal::Builtins;
/// }
/// # fn main() {}
/// ```
///
/// Downstream code can now `use my_crate::public_api::Builtins;` and use
/// it in `Resolver::new(...)` or as a `#[spread]` field, exactly as if
/// `#[base]` had been applied at `public_api::Builtins`.
///
/// # Generated code
///
/// The macro keeps the `pub type` alias and additionally emits a hidden
/// helper module at the re-export path. That module invokes the original
/// base's `@reexport` arm, which republishes every field-type re-export
/// (`Base_PartN_FieldType`, spread re-exports, `__generator`) under the
/// new path and rebuilds the `@impls` / `@insert` / `@reexport` arms
/// there. `#[spread]`-ing a re-exported base, or chaining
/// `#[reexport_base]` through several modules, therefore works
/// transparently.
pub use autoresolve_macros::reexport_base;
#[cfg(feature = "macros")]
#[doc(inline)]
/// Marks an `impl` block as a resolvable service: a type the framework can
/// construct on demand from its declared dependencies.
///
/// See [Defining services](crate#defining-services) in the crate-level
/// docs for the high-level overview. The block must contain a `fn new(...)`
/// whose parameters are shared references (`&Type`) and whose return type
/// is `Self`. Each parameter becomes one declared dependency. Other
/// inherent methods on the block are preserved verbatim.
///
/// # Example
///
/// ```
/// # #[derive(Clone)]
/// # pub struct Validator;
/// # #[derive(Clone)]
/// # pub struct Clock;
/// use autoresolve::resolvable;
///
/// pub struct Client {
///     validator: Validator,
///     clock: Clock,
/// }
///
/// #[resolvable]
/// impl Client {
///     fn new(validator: &Validator, clock: &Clock) -> Self {
///         Self { validator: validator.clone(), clock: clock.clone() }
///     }
///
///     // Other methods don't impact resolution.
///     pub fn number(&self) -> i32 { 0 }
/// }
/// # fn main() {}
/// ```
///
/// Once annotated, `Client` can be obtained from any [`Resolver<B>`] whose
/// base `B` transitively supplies `Validator` and `Clock` (via
/// [`resolver.get::<Client>()`](Resolver::get)).
///
/// # Generated code
///
/// The original `impl` block is preserved unchanged. The macro additionally
/// emits, for each dependency type `D`, a marker
/// `impl ::autoresolve::DependencyOf<Self> for D {}`, plus a generic
/// [`ResolveFrom`] impl roughly of the form:
///
/// ```ignore
/// impl<B> ::autoresolve::ResolveFrom<B> for Client
/// where
///     B: Send + Sync + 'static,
///     Validator: ::autoresolve::ResolveFrom<B>,
///     Clock: ::autoresolve::ResolveFrom<B>,
/// {
///     type Inputs = ::autoresolve::ResolutionDepsNode<
///         Validator,
///         ::autoresolve::ResolutionDepsNode<Clock, ::autoresolve::ResolutionDepsEnd>,
///     >;
///     fn new(inputs: <Self::Inputs as ::autoresolve::ResolutionDeps<B>>::Resolved) -> Self {
///         /* destructure the heterogeneous list and forward to Client::new(...) */
///     }
/// }
/// ```
///
/// Three things follow from this shape:
///
/// - **Generic over the base.** The same service participates in any
///   resolver whose base can supply its dependencies — there is no
///   per-resolver wiring step.
/// - **Transitive checking is free.** The compiler's trait solver verifies
///   the full dependency graph through the where-bounds. A missing
///   dependency surfaces as a `trait bound … is not satisfied` error with
///   a chain of `required for …` notes; a dependency cycle becomes an
///   overflow evaluating the requirement.
/// - **No imports required at the use site.** All paths in the generated
///   code are fully qualified (`::autoresolve::…`).
///
/// The [`DependencyOf`] markers exist so [`Resolver::provide`] /
/// `when_injected_in::<T>()` chains can be statically checked: extending
/// the chain with `T` requires the new head to be a declared dependency
/// of `T`.
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
