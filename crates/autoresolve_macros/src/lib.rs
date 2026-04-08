//! Proc macros for the `autoresolve` compile-time dependency injection framework.
//!
//! See the `autoresolve` crate for documentation and examples.

/// Marks an `impl` block as participating in the autoresolve dependency injection system.
///
/// The `fn new(...)` method in the block defines the dependency list. Each parameter must be a
/// shared reference `&Type`. The macro generates a generic `ResolveFrom<B>` impl that allows
/// this type to be automatically resolved by any `Resolver` whose base types transitively
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

/// Declares a module containing a resolver base type, generating the wiring needed to construct a
/// `Resolver` from it.
///
/// The module must contain exactly one struct with named fields. The macro generates hidden
/// re-exports (`__PartN`) so that the generated `macro_rules!` arms can reference field types via
/// `$crate::mod_name::__PartN`, making the generated code independent of import paths at the call
/// site.
///
/// # Primary base
///
/// Applied without arguments, `#[base]` generates:
/// - `ResolveFrom<Base>` impls for each field type (`#[spread]` fields delegate to their `@impls` arm).
/// - A `BaseType` impl that builds a `Resolver` by inserting all fields.
/// - A same-named declarative macro with `@impls` and `@insert` arms for use with `#[spread]` and `resolver!`.
/// - Hidden `__PartN` re-exports and friendly re-exports of field types.
///
/// Fields annotated with `#[spread]` are treated as spreadable base types — their individual
/// parts are spread into the resolver via the type's generated macro arms.
///
/// ```ignore
/// #[autoresolve::base]
/// mod app_base {
///     pub struct AppBase {
///         #[spread]
///         pub builtins: super::Builtins,
///         pub telemetry: super::Telemetry,
///     }
/// }
/// use app_base::AppBase;
///
/// let resolver = Resolver::new(AppBase { builtins, telemetry });
/// ```
///
/// # Scoped roots
///
/// With `scoped(ParentBase)`, the macro generates `ResolveFrom<ScopedBase>` impls for each
/// field type and sets `BaseType::Parent` to the parent, declaring its fields as root types
/// that will be pre-inserted into scoped resolvers.
///
/// ```ignore
/// #[autoresolve::base(scoped(AppBase))]
/// mod request_base {
///     use super::app_base::AppBase;
///     pub struct RequestBase {
///         pub request: super::Request,
///     }
/// }
/// use request_base::RequestBase;
/// ```
#[proc_macro_attribute]
pub fn base(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::base(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Re-exports a base type defined in another module, generating a new helper module at the
/// re-export site.
///
/// Use this when a `#[base]` struct is defined in a private module but needs to be
/// publicly accessible. The macro creates a `pub type` alias and a new helper module
/// whose generated macro arms reference the re-export path.
#[proc_macro_attribute]
pub fn reexport_base(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::reexport_base(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
