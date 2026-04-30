//! Proc macros for the [`autoresolve`] compile-time dependency injection
//! framework. See the [`autoresolve`] crate for an overview, examples, and
//! the design rationale; this crate documents each macro's surface and the
//! shape of the code it expands to.
//!
//! [`autoresolve`]: https://docs.rs/autoresolve

/// Marks an `impl` block as a [resolvable service]: a type the framework
/// can construct on demand from its declared dependencies.
///
/// [resolvable service]: https://docs.rs/autoresolve/latest/autoresolve/#defining-services
///
/// The block must contain a `fn new(...)` whose parameters are shared
/// references (`&Type`) and whose return type is `Self`. Each parameter
/// becomes one declared dependency. Other inherent methods on the block are
/// preserved verbatim.
///
/// # Example
///
/// ```ignore
/// use autoresolve::resolvable;
///
/// pub struct Client { /* ... */ }
///
/// #[resolvable]
/// impl Client {
///     fn new(validator: &Validator, config: &Config) -> Self {
///         Self { validator: validator.clone(), config: config.clone() }
///     }
///
///     pub fn number(&self) -> i32 { /* ... */ }
/// }
/// ```
///
/// Once annotated, `Client` can be obtained from any `Resolver<B>` whose base
/// `B` transitively supplies `Validator` and `Config`:
///
/// ```ignore
/// let client: std::sync::Arc<Client> = resolver.get::<Client>();
/// ```
///
/// # Generated code
///
/// The original `impl` block is preserved unchanged. The macro additionally
/// emits, for each dependency type `D`, a marker
/// `impl ::autoresolve::DependencyOf<Self> for D {}`, plus a generic
/// `ResolveFrom` impl roughly of the form:
///
/// ```ignore
/// impl<B> ::autoresolve::ResolveFrom<B> for Client
/// where
///     B: Send + Sync + 'static,
///     Validator: ::autoresolve::ResolveFrom<B>,
///     Config: ::autoresolve::ResolveFrom<B>,
/// {
///     type Inputs = ::autoresolve::ResolutionDepsNode<
///         Validator,
///         ::autoresolve::ResolutionDepsNode<Config, ::autoresolve::ResolutionDepsEnd>,
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
///   dependency surfaces as a `trait bound … is not satisfied` error with a
///   chain of `required for …` notes; a dependency cycle becomes an
///   overflow evaluating the requirement.
/// - **No imports required at the use site.** All paths in the generated
///   code are fully qualified (`::autoresolve::…`).
///
/// The `DependencyOf` markers exist so [`Resolver::provide`] /
/// `when_injected_in::<T>()` chains can be statically checked: extending the
/// chain with `T` requires the new head to be a declared dependency of `T`.
///
/// [`Resolver::provide`]: https://docs.rs/autoresolve/latest/autoresolve/struct.Resolver.html#method.provide
#[proc_macro_attribute]
pub fn resolvable(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::resolvable(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Declares a struct as a resolver *base* — the set of root values from
/// which a `Resolver` (or a [`scoped`](#scoped-bases) child resolver) is
/// built.
///
/// See [Custom Bases] in the crate-level `autoresolve` docs for the
/// conceptual overview. This page documents the macro's surface and the
/// code it expands to.
///
/// [Custom Bases]: https://docs.rs/autoresolve/latest/autoresolve/#custom-bases
///
/// # Form
///
/// `#[base]` is applied to a struct with named fields:
///
/// ```ignore
/// use autoresolve::base;
///
/// #[base(helper_module_exported_as = crate::app_base_helper)]
/// pub struct AppBase {
///     pub scheduler: Scheduler,
///     pub clock: Clock,
/// }
/// ```
///
/// The `helper_module_exported_as = crate::path::to::helper` argument is
/// **required**. It names the `crate::`-rooted absolute path at which the
/// macro will publish a hidden helper module containing the wiring used by
/// `#[spread]` and by [`#[reexport_base]`]. The path must match the module
/// in which `#[base]` is invoked (e.g. for a base defined at the crate root,
/// pass `crate::app_base_helper`; inside `mod runtime`, pass
/// `crate::runtime::app_base_helper`).
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
/// ```ignore
/// // In a runtime crate:
/// #[base(helper_module_exported_as = crate::builtins_helper)]
/// pub struct Builtins {
///     pub scheduler: Scheduler,
///     pub clock: Clock,
/// }
///
/// // In an application crate:
/// #[base(helper_module_exported_as = crate::app_base_helper)]
/// pub struct AppBase {
///     #[spread]
///     pub builtins: my_runtime::Builtins,
///     pub app_context: AppContext,
/// }
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
/// [`Resolver::scoped`]; see [Scoped Bases].
///
/// [`Resolver::scoped`]: https://docs.rs/autoresolve/latest/autoresolve/struct.Resolver.html#method.scoped
/// [Scoped Bases]: https://docs.rs/autoresolve/latest/autoresolve/#scoped-bases
///
/// ```ignore
/// #[base(scoped(AppBase), helper_module_exported_as = crate::request_base_helper)]
/// pub struct RequestBase {
///     pub request: Request,
/// }
///
/// let mut req = app.scoped(RequestBase { request });
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
///   constructed); the `@insert` arm destructures the base struct and pushes
///   each field into the resolver.
/// - An invocation of the `@impls` arm that materializes the
///   `ResolveFrom<Self>` impls.
/// - An [`autoresolve::BaseType`] impl whose `Parent` associated type is `()`
///   for primary bases and `ParentBase` for scoped ones, and whose
///   `insert_into` calls the `@insert` arm.
///
/// [`autoresolve::BaseType`]: https://docs.rs/autoresolve/latest/autoresolve/trait.BaseType.html
///
/// The mangling and helper-module redirection exist so that `#[spread]`
/// (and `#[reexport_base]`) can refer to a foreign base's parts purely by
/// macro path, without requiring the downstream crate to import every
/// individual field type.
///
/// [`#[reexport_base]`]: macro@reexport_base
#[proc_macro_attribute]
pub fn base(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::base(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Re-exports a base type defined in another module so that downstream code
/// can refer to it as if `#[base]` had been applied locally.
///
/// `#[base]` publishes its wiring under a hidden helper module rooted at a
/// `crate::`-rooted absolute path. When a base is `pub use`-d into another
/// module, that path no longer matches the new location — `#[spread]`
/// references and friendly re-exports would be looking for the helper at the
/// wrong place. `#[reexport_base]` solves this by emitting a fresh helper
/// module at the re-export site that forwards to the original.
///
/// # Form
///
/// Apply the attribute to a `pub type` alias whose right-hand side names the
/// original `#[base]` struct, supplying a `helper_module_exported_as` path
/// pointing at the *new* location:
///
/// ```ignore
/// // Original `#[base]` lives in a private module inside `my_runtime`:
/// mod internal {
///     #[autoresolve::base(helper_module_exported_as = crate::internal::builtins_helper)]
///     pub struct Builtins {
///         pub scheduler: Scheduler,
///         pub clock: Clock,
///     }
/// }
///
/// pub mod core {
///     // Re-export it as `core::Builtins` so external crates use a clean path.
///     #[autoresolve::reexport_base(helper_module_exported_as = crate::core::builtins_helper)]
///     pub type Builtins = super::internal::Builtins;
/// }
/// ```
///
/// Downstream code can now `use my_runtime::core::Builtins;` and use it in
/// `Resolver::new(...)` or as a `#[spread]` field, exactly as if `#[base]`
/// had been applied at `core::Builtins`.
///
/// # Generated code
///
/// The macro keeps the `pub type` alias and additionally emits a hidden
/// `helper` module at the re-export path. That module invokes the original
/// base's `@reexport` arm, which republishes every field-type re-export
/// (`Base_PartN_FieldType`, spread re-exports, `__generator`) under the new
/// path and rebuilds the `@impls` / `@insert` / `@reexport` arms there.
/// `#[spread]`-ing a re-exported base, or chaining `#[reexport_base]`
/// through several modules, therefore works transparently.
#[proc_macro_attribute]
pub fn reexport_base(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::reexport_base(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
