//! Proc macros for the [`autoresolve`](https://docs.rs/autoresolve)
//! compile-time dependency injection framework.
//!
//! These macros are re-exported from the `autoresolve` crate when its
//! `macros` feature is enabled (the default). Refer to that crate's
//! documentation for usage, examples, and the design rationale — the
//! re-export sites carry the full docs.

#[expect(missing_docs, reason = "this is documented in the autoresolve reexport")]
#[proc_macro_attribute]
pub fn resolvable(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::resolvable(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the autoresolve reexport")]
#[proc_macro_attribute]
pub fn base(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::base(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the autoresolve reexport")]
#[proc_macro_attribute]
pub fn reexport_base(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    autoresolve_macros_impl::reexport_base(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
