//! Proc macros for the `autoresolve` compile-time dependency injection framework.
//!
//! See the [`autoresolve`] crate for documentation and examples.

/// Marks a struct as a composite type whose fields are individually injected into the resolver.
///
/// The `#[composite]` attribute generates:
/// - `CompositePart<N>` trait impls mapping each field index to its type.
/// - A same-named declarative macro used internally by `resolver!` to register all field types.
///
/// # Example
///
/// ```ignore
/// #[composite]
/// struct Builtins {
///     scheduler: Scheduler,
///     clock: Clock,
/// }
/// ```
///
/// Then in `resolver!`, use `..name: Type` to decompose the composite:
///
/// ```ignore
/// let mut resolver = autoresolve::resolver!(MyBase,
///     ..builtins: Builtins,
/// );
/// ```
#[proc_macro_attribute]
pub fn composite(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::composite(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Marks an `impl` block as participating in the autoresolve dependency injection system.
///
/// The `fn new(...)` method in the block defines the dependency list. Each parameter must be a
/// shared reference `&Type`. The macro generates a generic `ResolveFrom<B>` impl that allows
/// this type to be automatically resolved by any [`Resolver`] whose base types transitively
/// satisfy all dependencies.
///
/// # Example
///
/// ```ignore
/// #[resolvable]
/// impl Client {
///     fn new(validator: &Validator, config: &Config) -> Self {
///         Self { validator: validator.clone(), config: config.clone() }
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn resolvable(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::resolvable(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Declares a struct as a resolver base type, generating the wiring needed to construct a
/// [`Resolver`] from it.
///
/// # Primary base
///
/// Applied without arguments, `#[base]` generates:
/// - `ResolveFrom<Base>` impls for each field type (composite fields use their `@impls` arm).
/// - A [`BaseType`] impl that builds a [`Resolver`] by inserting all fields.
///
/// Fields annotated with `#[spread]` are treated as composite types — their individual parts
/// are spread into the resolver via the composite's generated macro.
///
/// ```ignore
/// #[autoresolve::base]
/// struct Base {
///     #[spread]
///     builtins: Builtins,
///     telemetry: Telemetry,
/// }
///
/// let resolver = Resolver::new(Base { builtins, telemetry });
/// ```
///
/// # Scoped roots
///
/// With `scoped(ParentBase)`, the macro generates `ResolveFrom<ParentBase>` impls for each
/// field type, declaring them as root types that will be pre-inserted into scoped resolvers.
///
/// ```ignore
/// #[autoresolve::base(scoped(Base))]
/// struct ScopedRoots {
///     request_context: RequestContext,
/// }
/// ```
#[proc_macro_attribute]
pub fn base(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::base(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
