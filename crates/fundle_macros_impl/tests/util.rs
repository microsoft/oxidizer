#[macro_export]
macro_rules! expand_fundle_bundle {
    ($item:expr) => {{
        // Helper function to check if a path matches "bundle" or "fundle::bundle"
        fn is_bundle_attr(path: &syn::Path) -> bool {
            if path.is_ident("bundle") {
                return true;
            }
            if path.segments.len() == 2
                && path.segments[0].ident == "fundle"
                && path.segments[1].ident == "bundle" {
                return true;
            }
            false
        }

        // Extract just the arguments from inside the attribute (empty in this case)
        let item_ref = &$item;
        let attr_args = if let Some(attr) = item_ref.attrs.iter().find(|a| is_bundle_attr(a.path())) {
            match &attr.meta {
                syn::Meta::Path(_) => quote::quote! {},              // #[bundle] or #[fundle::bundle] with no args
                syn::Meta::List(list) => list.tokens.clone(),        // #[bundle(...)] or #[fundle::bundle(...)]
                syn::Meta::NameValue(_) => quote::quote! {},         // shouldn't happen for your use case
            }
        } else {
            quote::quote! {}
        };

        // Recreate struct without the bundle attribute by building new tokens
        let attrs = item_ref.attrs.iter().filter(|attr| !is_bundle_attr(attr.path()));
        let vis = &item_ref.vis;
        let struct_token = &item_ref.struct_token;
        let ident = &item_ref.ident;
        let generics = &item_ref.generics;
        let fields = &item_ref.fields;
        let semi_token = &item_ref.semi_token;

        let item_tokens = quote::quote! {
            #(#attrs)*
            #vis #struct_token #ident #generics #fields #semi_token
        };

        let output = fundle_macros_impl::bundle(attr_args, item_tokens)
            .unwrap_or_else(|e| e.to_compile_error());

        prettyplease::unparse(&syn::parse2(output).unwrap())
    }};
}