// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test utilities for snapshot testing the `thunk` macro expansion.

/// Helper macro to expand a `#[thunk(...)]` attribute on an `async fn` item
/// inside an `impl` block and pretty-print the result for snapshot comparison.
#[macro_export]
macro_rules! expand_thunk {
    ($item:expr) => {{
        let item_fn: &syn::ImplItemFn = &$item;

        // Extract the thunk attribute arguments.
        let attr = item_fn
            .attrs
            .iter()
            .find(|a| a.path().is_ident("thunk"))
            .expect("item must have a #[thunk(...)] attribute");
        let attr_args = match &attr.meta {
            syn::Meta::Path(_) => quote::quote! {},
            syn::Meta::List(list) => list.tokens.clone(),
            syn::Meta::NameValue(_) => quote::quote! {},
        };

        // Strip the thunk attribute before passing to the macro.
        let mut clean = item_fn.clone();
        clean.attrs.retain(|a| !a.path().is_ident("thunk"));
        let item_tokens = quote::quote! { #clean };

        let output = sync_thunk_macros_impl::thunk_impl(attr_args, item_tokens).unwrap_or_else(|e| e.to_compile_error());

        // Wrap in a dummy impl block so prettyplease can parse it.
        let wrapped = quote::quote! { impl Dummy { #output } };
        let file: syn::File = syn::parse2(wrapped).unwrap();
        prettyplease::unparse(&file)
    }};
}

/// Helper macro for cases that should produce a compile error.
#[macro_export]
macro_rules! expand_thunk_error {
    ($attr_args:expr, $item_tokens:expr) => {{
        let output = sync_thunk_macros_impl::thunk_impl($attr_args, $item_tokens).unwrap_or_else(|e| e.to_compile_error());

        let wrapped = quote::quote! { impl Dummy { #output } };
        let file: syn::File = syn::parse2(wrapped).unwrap();
        prettyplease::unparse(&file)
    }};
}
