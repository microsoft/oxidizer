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
