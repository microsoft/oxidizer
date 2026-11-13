// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![forbid(unsafe_code)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Expands Arty's runtime entry-point attributes.
//!
//! This crate implements the attributes from
//! [`arty_macros`](https://docs.rs/arty_macros). Application code uses Arty's
//! `macros` feature and its `main` and `test` attributes.
//!
//! [`main()`] turns an async function into a synchronous entry point. [`test()`]
//! also registers the function with Rust's test harness. Invalid input produces
//! compiler diagnostic tokens.

use darling::FromMeta;
use darling::ast::NestedMeta;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::ext::IdentExt;
use syn::{Expr, Ident, ItemFn, Lit, parse_quote, parse2};

#[derive(Debug, FromMeta)]
struct Args {
    #[darling(default)]
    monitor: Option<MonitorSetting>,
    #[darling(default)]
    panic: Option<PanicSetting>,
    #[darling(default, with = "darling::util::parse_expr::preserve_str_literal", map = "Some")]
    workers: Option<Expr>,
}

#[derive(Clone, Copy, Debug)]
enum MonitorSetting {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug)]
enum PanicSetting {
    Shutdown,
    Ignore,
}

impl FromMeta for MonitorSetting {
    fn from_expr(expr: &Expr) -> darling::Result<Self> {
        let Expr::Path(path) = expr else {
            return Err(invalid_monitor_setting(expr));
        };
        let Some(ident) = path.path.get_ident() else {
            return Err(invalid_monitor_setting(expr));
        };
        match ident.to_string().as_str() {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            _ => Err(invalid_monitor_setting(expr)),
        }
    }
}

fn invalid_monitor_setting(expr: &Expr) -> darling::Error {
    darling::Error::custom("`monitor` must be the bare identifier `enabled` or `disabled`").with_span(expr)
}

impl FromMeta for PanicSetting {
    fn from_expr(expr: &Expr) -> darling::Result<Self> {
        let Expr::Path(path) = expr else {
            return Err(invalid_panic_setting(expr));
        };
        let Some(ident) = path.path.get_ident() else {
            return Err(invalid_panic_setting(expr));
        };
        match ident.to_string().as_str() {
            "shutdown" => Ok(Self::Shutdown),
            "ignore" => Ok(Self::Ignore),
            _ => Err(invalid_panic_setting(expr)),
        }
    }
}

fn invalid_panic_setting(expr: &Expr) -> darling::Error {
    darling::Error::custom("`panic` must be the bare identifier `shutdown` or `ignore`").with_span(expr)
}

impl Args {
    fn runtime(&self, runtime_path: &TokenStream, clock: Option<&Ident>, test: bool) -> syn::Result<TokenStream> {
        let monitor = self.monitor.or_else(|| test.then_some(MonitorSetting::Disabled));
        if self.workers.is_none() && clock.is_none() && monitor.is_none() && self.panic.is_none() {
            return Ok(quote!(#runtime_path::runtime::Runtime::new()));
        }
        let workers = self.workers.as_ref().map(|value| worker_policy(value, runtime_path)).transpose()?;
        let workers = workers.map(|policy| quote!(.workers(#policy)));
        let monitor = monitor.map(|monitor| match monitor {
            MonitorSetting::Enabled => quote!(.monitor(#runtime_path::runtime::MonitorPolicy::enabled())),
            MonitorSetting::Disabled => quote!(.monitor(#runtime_path::runtime::MonitorPolicy::disabled())),
        });
        let panic = self.panic.map(|panic| match panic {
            PanicSetting::Shutdown => quote!(.on(#runtime_path::runtime::hook::Panic::SHUTDOWN)),
            PanicSetting::Ignore => quote!(.on(#runtime_path::runtime::hook::Panic::IGNORE)),
        });
        let clock = clock.map(|binding| quote!(.clock(::core::clone::Clone::clone(&#binding))));
        Ok(quote!(#runtime_path::runtime::Runtime::builder() #workers #monitor #panic #clock .build()))
    }
}

fn arty_path() -> TokenStream {
    arty_path_for(crate_name("arty").ok())
}

fn arty_path_for(found: Option<FoundCrate>) -> TokenStream {
    match found {
        Some(FoundCrate::Name(name)) => {
            let name = name.replace('-', "_");
            let name = syn::parse_str::<Ident>(&name)
                .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
                .expect("Cargo dependency aliases are valid Rust identifiers, possibly requiring raw syntax");
            quote!(::#name)
        }
        Some(FoundCrate::Itself) | None => quote!(::arty),
    }
}

fn worker_policy(mut value: &Expr, runtime_path: &TokenStream) -> syn::Result<TokenStream> {
    while let Expr::Group(group) = value {
        value = &group.expr;
    }
    if let Expr::Lit(syn::ExprLit { lit: Lit::Int(count), .. }) = value
        && !count.base10_digits().starts_with('-')
        && matches!(count.suffix(), "" | "usize")
    {
        return Ok(quote!(#runtime_path::runtime::WorkersPolicy::exactly(#count)));
    }
    if let Expr::Path(syn::ExprPath { qself: None, path, .. }) = value
        && path.is_ident("all")
    {
        return Ok(quote!(#runtime_path::runtime::WorkersPolicy::all()));
    }
    Err(syn::Error::new_spanned(
        value,
        "`workers` must be a nonnegative integer literal, optionally suffixed with `usize`, or `all`; use explicit runtime construction for expressions or other processor policies",
    ))
}

/// Expands an async function into a synchronous Arty entry point.
///
/// `args` contains the attribute configuration. `item` contains the annotated
/// async function. Valid input returns tokens for the entry point. Invalid input
/// returns compiler diagnostic tokens.
///
/// This function implements `arty_macros::main`. Application code uses
/// [`arty::main`](https://docs.rs/arty/latest/arty/attr.main.html), which
/// documents the configuration and the generated function.
///
/// # Examples
///
/// ```
/// use quote::quote;
///
/// let expanded = arty_macros_impl::main(
///     quote!(),
///     quote!(
///         async fn main(cx: arty::task::Context) {}
///     ),
/// );
/// assert!(!expanded.is_empty());
/// ```
#[must_use]
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    entrypoint(args, item, false)
}

/// Expands an async function into a synchronous Arty test.
///
/// `args` contains the attribute configuration. `item` contains the annotated
/// async function. Valid input returns tokens that register the test with Rust's
/// test harness. Invalid input returns compiler diagnostic tokens.
///
/// This function implements `arty_macros::test`. Application code uses
/// [`arty::test`](https://docs.rs/arty/latest/arty/attr.test.html), which
/// documents examples and the optional controlled-time argument.
///
/// # Examples
///
/// ```
/// use quote::quote;
///
/// let expanded = arty_macros_impl::test(
///     quote!(),
///     quote!(
///         async fn checks_answer(cx: arty::task::Context) {
///             assert_eq!(cx.scheduler().spawn(async |_| 42).await.unwrap(), 42);
///         }
///     ),
/// );
/// assert!(!expanded.is_empty());
/// ```
#[must_use]
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    entrypoint(args, item, true)
}

fn entrypoint(args: TokenStream, item: TokenStream, test: bool) -> TokenStream {
    let mut input: ItemFn = match parse2(item.clone()) {
        Ok(input) => input,
        Err(error) => return error.to_compile_error(),
    };
    let args = match NestedMeta::parse_meta_list(args) {
        Ok(args) => args,
        Err(error) => return darling::Error::from(error).write_errors(),
    };
    let args = match Args::from_list(&args) {
        Ok(args) => args,
        Err(error) => return error.write_errors(),
    };
    let runtime_path = arty_path();
    let sig = &mut input.sig;
    let mut inputs = sig.inputs.iter();
    let fail = move |error: syn::Error| {
        let error = error.to_compile_error();
        quote! { #item #error }
    };
    if sig.asyncness.is_none() {
        return fail(syn::Error::new_spanned(sig.fn_token, "function must be async to use the attribute"));
    }
    let Some(syn::FnArg::Typed(state)) = inputs.next() else {
        return fail(syn::Error::new_spanned(
            sig.fn_token,
            "function must take an Arty task context argument, such as cx: arty::task::Context",
        ));
    };
    let syn::Pat::Ident(state_ident) = state.pat.as_ref() else {
        return fail(syn::Error::new_spanned(&state.pat, "argument must have an identifier"));
    };
    let syn::Type::Path(state_type) = state.ty.as_ref() else {
        return fail(syn::Error::new_spanned(
            &state.ty,
            "the first parameter must take arty::task::Context by value, such as cx: arty::task::Context",
        ));
    };
    let state_ident = state_ident.clone();
    let state_type = state_type.clone();
    let clock = match inputs.next() {
        Some(syn::FnArg::Typed(clock)) if test && matches!(clock.ty.as_ref(), syn::Type::Path(_)) => {
            let syn::Pat::Ident(ident) = clock.pat.as_ref() else {
                return fail(syn::Error::new_spanned(&clock.pat, "argument must have an identifier"));
            };
            if ident.ident.unraw() == state_ident.ident.unraw() {
                return fail(syn::Error::new_spanned(ident, "arguments must have distinct identifiers"));
            }
            Some((ident.clone(), clock.ty.clone()))
        }
        Some(extra) => return fail(syn::Error::new_spanned(extra, "unexpected arguments")),
        None => None,
    };
    if let Some(extra) = inputs.next() {
        return fail(syn::Error::new_spanned(extra, "unexpected arguments"));
    }
    let clock_binding = Ident::new("__arty_clock_control", Span::mixed_site());
    let runtime_binding = Ident::new("__arty_runtime", Span::mixed_site());
    let result_binding = Ident::new("__arty_result", Span::mixed_site());
    let shutdown_binding = Ident::new("__arty_shutdown", Span::mixed_site());
    let value_binding = Ident::new("__arty_value", Span::mixed_site());
    let runtime = match args.runtime(&runtime_path, clock.as_ref().map(|_| &clock_binding), test) {
        Ok(runtime) => runtime,
        Err(error) => return fail(error),
    };
    sig.asyncness = None;
    sig.inputs.clear();
    let mut attrs = input.attrs;
    if test {
        attrs.push(parse_quote!(#[::core::prelude::v1::test]));
    }
    let visibility = input.vis;
    let body = input.block;
    let (setup, body) = if let Some((ident, ty)) = clock {
        (
            quote!(let #clock_binding = #runtime_path::runtime::__private::ClockControl::new();),
            quote!({
                let #ident: #ty = #clock_binding;
                #body
            }),
        )
    } else {
        (TokenStream::new(), quote!(#body))
    };
    quote! {
        #(#attrs)*
        #visibility #sig {
            #setup
            let #runtime_binding = #runtime
                .expect("failed to create the runtime for the entry point");
            let #result_binding = #runtime_binding
                .scheduler()
                .block_on(async move |#state_ident: #state_type| #body);
            let #shutdown_binding = #runtime_binding.stop();
            #result_binding
                .and_then(|#value_binding| #shutdown_binding.map(|()| #value_binding))
                .unwrap_or_else(|error| #runtime_path::runtime::__private::resume_error(error))
        }
    }
}

#[cfg(test)]
mod tests {
    // Dependency discovery branches require private injected Cargo metadata.

    use super::*;

    #[test]
    fn arty_path_uses_a_renamed_dependency() {
        assert_eq!(
            arty_path_for(Some(FoundCrate::Name("renamed-arty".to_owned()))).to_string(),
            ":: renamed_arty"
        );
        assert_eq!(arty_path_for(Some(FoundCrate::Itself)).to_string(), ":: arty");
        assert_eq!(arty_path_for(None).to_string(), ":: arty");
    }

    #[test]
    fn arty_path_supports_a_keyword_dependency_alias() {
        assert_eq!(arty_path_for(Some(FoundCrate::Name("type".to_owned()))).to_string(), ":: r#type");
    }
}
