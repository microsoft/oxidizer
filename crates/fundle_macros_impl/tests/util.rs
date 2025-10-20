#[macro_export]
macro_rules! expand_fundle_bundle {
    ($item:expr) => {{
        // Extract just the arguments from inside the attribute (empty in this case)
        let attr_args = if let Some(attr) = $item.attrs.iter().find(|a| a.path().is_ident("bundle")) {
            match &attr.meta {
                syn::Meta::Path(_) => quote::quote! {},       // #[ffi] with no args
                syn::Meta::List(list) => list.tokens.clone(), // #[ffi(...)]
                syn::Meta::NameValue(_) => quote::quote! {},  // shouldn't happen for your use case
            }
        } else {
            quote::quote! {}
        };

        // Create a clean item without the bundle attribute
        let mut clean_item = $item.clone();
        clean_item.attrs.retain(|attr| !attr.path().is_ident("bundle"));
        let item_tokens = quote::quote! { #clean_item };

        let output = fundle_macros_impl::bundle(attr_args, item_tokens)
            .unwrap_or_else(|e| e.to_compile_error());

        prettyplease::unparse(&syn::parse2(output).unwrap())
    }};
}


#[macro_export]
macro_rules! expand_fundle_deps {
    ($item:expr) => {{
        // Extract just the arguments from inside the attribute (empty in this case)
        let attr_args = if let Some(attr) = $item.attrs.iter().find(|a| a.path().is_ident("deps")) {
            match &attr.meta {
                syn::Meta::Path(_) => quote::quote! {},       // #[ffi] with no args
                syn::Meta::List(list) => list.tokens.clone(), // #[ffi(...)]
                syn::Meta::NameValue(_) => quote::quote! {},  // shouldn't happen for your use case
            }
        } else {
            quote::quote! {}
        };

        // Create a clean item without the bundle attribute
        let mut clean_item = $item.clone();
        clean_item.attrs.retain(|attr| !attr.path().is_ident("deps"));
        let item_tokens = quote::quote! { #clean_item };

        let output = fundle_macros_impl::deps(attr_args, item_tokens)
            .unwrap_or_else(|e| e.to_compile_error());

        prettyplease::unparse(&syn::parse2(output).unwrap())
    }};
}

#[macro_export]
macro_rules! expand_fundle_newtype {
    ($item:expr) => {{
        // Extract just the arguments from inside the attribute (empty in this case)
        let attr_args = if let Some(attr) = $item.attrs.iter().find(|a| a.path().is_ident("newtype")) {
            match &attr.meta {
                syn::Meta::Path(_) => quote::quote! {},       // #[ffi] with no args
                syn::Meta::List(list) => list.tokens.clone(), // #[ffi(...)]
                syn::Meta::NameValue(_) => quote::quote! {},  // shouldn't happen for your use case
            }
        } else {
            quote::quote! {}
        };

        // Create a clean item without the bundle attribute
        let mut clean_item = $item.clone();
        clean_item.attrs.retain(|attr| !attr.path().is_ident("newtype"));
        let item_tokens = quote::quote! { #clean_item };

        let output = fundle_macros_impl::newtype(attr_args, item_tokens)
            .unwrap_or_else(|e| e.to_compile_error());

        prettyplease::unparse(&syn::parse2(output).unwrap())
    }};
}