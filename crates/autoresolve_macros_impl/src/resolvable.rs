use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{FnArg, ImplItem, ItemImpl, Pat, ReturnType, Type};

/// Implements the `#[resolvable]` attribute macro.
///
/// Parses an inherent `impl` block containing `fn new(...)`, then generates a
/// `ResolveFrom<B>` blanket impl that wires the constructor's `&Type` parameters
/// into the resolver's dependency graph.
pub fn resolvable(_attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let impl_block: ItemImpl = syn::parse2(item)?;

    // Must be an inherent impl (not a trait impl).
    if impl_block.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            &impl_block,
            "#[resolvable] can only be applied to inherent impl blocks, not trait impls",
        ));
    }

    // Must not have generic parameters (the type itself can be generic in theory,
    // but we don't support that yet).
    if !impl_block.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &impl_block.generics,
            "#[resolvable] does not support generic impl blocks",
        ));
    }

    let self_ty = &impl_block.self_ty;

    // Find the `fn new(...)` method.
    let new_fn = impl_block
        .items
        .iter()
        .find_map(|item| {
            if let ImplItem::Fn(method) = item {
                if method.sig.ident == "new" {
                    return Some(method);
                }
            }
            None
        })
        .ok_or_else(|| syn::Error::new_spanned(&impl_block, "#[resolvable] requires a `fn new(...)` method in the impl block"))?;

    // Must not have a self receiver.
    if new_fn.sig.receiver().is_some() {
        return Err(syn::Error::new_spanned(
            &new_fn.sig,
            "#[resolvable] `fn new()` must not have a self receiver",
        ));
    }

    // Return type must be Self.
    match &new_fn.sig.output {
        ReturnType::Type(_, ty) => {
            if let Type::Path(p) = ty.as_ref() {
                let is_self = p.qself.is_none() && p.path.segments.len() == 1 && p.path.segments[0].ident == "Self";
                if !is_self {
                    return Err(syn::Error::new_spanned(ty, "#[resolvable] `fn new()` must return `Self`"));
                }
            } else {
                return Err(syn::Error::new_spanned(ty, "#[resolvable] `fn new()` must return `Self`"));
            }
        }
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                &new_fn.sig,
                "#[resolvable] `fn new()` must have a return type of `Self`",
            ));
        }
    }

    // Extract dependency types from parameters — each must be `&Type`.
    let mut dep_types: Vec<&Type> = Vec::new();
    for arg in &new_fn.sig.inputs {
        match arg {
            FnArg::Receiver(_) => {
                // Already checked above, but be safe.
                return Err(syn::Error::new_spanned(
                    arg,
                    "#[resolvable] `fn new()` must not have a self receiver",
                ));
            }
            FnArg::Typed(pat_type) => {
                // Verify the parameter name is a simple ident (not a pattern).
                if let Pat::Ident(_) = pat_type.pat.as_ref() {
                    // ok
                } else {
                    return Err(syn::Error::new_spanned(
                        &pat_type.pat,
                        "#[resolvable] `fn new()` parameters must be simple identifiers",
                    ));
                }

                // The type must be a reference `&Type`.
                if let Type::Reference(ref_type) = pat_type.ty.as_ref() {
                    if ref_type.mutability.is_some() {
                        return Err(syn::Error::new(
                            ref_type.and_token.span(),
                            "#[resolvable] `fn new()` parameters must be shared references (&Type), not &mut",
                        ));
                    }
                    dep_types.push(ref_type.elem.as_ref());
                } else {
                    return Err(syn::Error::new_spanned(
                        &pat_type.ty,
                        "#[resolvable] `fn new()` parameters must be shared references (&Type)",
                    ));
                }
            }
        }
    }

    // Build the nested ResolutionDepsNode type.
    let inputs_type = build_deps_type(&dep_types);

    // Build the destructuring pattern.
    let dep_idents: Vec<_> = (0..dep_types.len()).map(|i| format_ident!("dep_{i}")).collect();
    let destructure = build_destructure_pattern(&dep_idents);

    // Build the where clause bounds.
    let where_bounds: Vec<_> = dep_types
        .iter()
        .map(|ty| {
            quote! { #ty: ::autoresolve::ResolveFrom<__AutoresolveBase> }
        })
        .collect();

    let generated = quote! {
        #impl_block

        #(
            impl ::autoresolve::DependencyOf<#self_ty> for #dep_types {}
        )*

        impl<__AutoresolveBase> ::autoresolve::ResolveFrom<__AutoresolveBase> for #self_ty
        where
            __AutoresolveBase: Send + Sync + 'static,
            #( #where_bounds, )*
        {
            type Inputs = #inputs_type;

            fn new(
                inputs: <Self::Inputs as ::autoresolve::ResolutionDeps<__AutoresolveBase>>::Resolved,
            ) -> Self {
                let #destructure = inputs;
                #self_ty::new(#( &#dep_idents ),*)
            }
        }
    };

    Ok(generated)
}

fn build_deps_type(dep_types: &[&Type]) -> TokenStream {
    let mut result = quote! { ::autoresolve::ResolutionDepsEnd };
    for ty in dep_types.iter().rev() {
        result = quote! {
            ::autoresolve::ResolutionDepsNode<#ty, #result>
        };
    }
    result
}

fn build_destructure_pattern(dep_idents: &[proc_macro2::Ident]) -> TokenStream {
    let mut result = quote! { ::autoresolve::ResolutionDepsEnd };
    for ident in dep_idents.iter().rev() {
        result = quote! {
            ::autoresolve::ResolutionDepsNode(#ident, #result)
        };
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pretty_print(tokens: TokenStream) -> String {
        let file = syn::parse2::<syn::File>(tokens).unwrap_or_else(|e| panic!("Failed to parse generated code: {e}"));
        prettyplease::unparse(&file)
    }

    /// Snapshot: type with one dependency.
    #[test]
    fn single_dependency() {
        let input = quote! {
            impl Validator {
                fn new(builtins: &Builtins) -> Self {
                    Self { builtins: builtins.clone() }
                }
            }
        };
        let result = resolvable(TokenStream::new(), input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    /// Snapshot: type with three dependencies and an extra method.
    #[test]
    fn multiple_dependencies() {
        let input = quote! {
            impl Client {
                fn new(validator: &Validator, builtins: &Builtins, telemetry: &Telemetry) -> Self {
                    Self {
                        validator: validator.clone(),
                        builtins: builtins.clone(),
                        telemetry: telemetry.clone(),
                    }
                }

                fn number(&self) -> i32 {
                    42
                }
            }
        };
        let result = resolvable(TokenStream::new(), input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    /// Snapshot: type with no dependencies.
    #[test]
    fn no_dependencies() {
        let input = quote! {
            impl Leaf {
                fn new() -> Self {
                    Self
                }
            }
        };
        let result = resolvable(TokenStream::new(), input).expect("should succeed");
        insta::assert_snapshot!(pretty_print(result));
    }

    /// Rejects an impl block without `fn new`.
    #[test]
    fn error_missing_new() {
        let input = quote! {
            impl Foo {
                fn build(x: &Bar) -> Self {
                    Self { x: x.clone() }
                }
            }
        };
        let result = resolvable(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires a `fn new("));
    }

    /// Rejects a `fn new` with a non-reference parameter.
    #[test]
    fn error_non_reference_param() {
        let input = quote! {
            impl Foo {
                fn new(x: Bar) -> Self {
                    Self { x }
                }
            }
        };
        let result = resolvable(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("shared references"));
    }

    /// Rejects a trait impl block.
    #[test]
    fn error_trait_impl() {
        let input = quote! {
            impl SomeTrait for Foo {
                fn new(x: &Bar) -> Self {
                    Self { x: x.clone() }
                }
            }
        };
        let result = resolvable(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("inherent impl blocks"));
    }

    /// Rejects a generic impl block.
    #[test]
    fn error_generic_impl() {
        let input = quote! {
            impl<T> Foo<T> {
                fn new() -> Self {
                    unimplemented!()
                }
            }
        };
        let result = resolvable(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not support generic impl blocks"));
    }

    /// Rejects a `fn new` with a `&self` receiver.
    #[test]
    fn error_self_receiver() {
        let input = quote! {
            impl Foo {
                fn new(&self) -> Self {
                    self.clone()
                }
            }
        };
        let result = resolvable(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must not have a self receiver"));
    }

    /// Rejects a `fn new` returning a type other than `Self`.
    #[test]
    fn error_wrong_return_type() {
        let input = quote! {
            impl Foo {
                fn new() -> Bar {
                    Bar
                }
            }
        };
        let result = resolvable(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must return `Self`"));
    }

    /// Rejects a `fn new` with no return type.
    #[test]
    fn error_no_return_type() {
        let input = quote! {
            impl Foo {
                fn new() {}
            }
        };
        let result = resolvable(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must have a return type of `Self`"));
    }

    /// Rejects a `fn new` with a `&mut` parameter.
    #[test]
    fn error_mut_ref_param() {
        let input = quote! {
            impl Foo {
                fn new(x: &mut Bar) -> Self {
                    Self { x: x.clone() }
                }
            }
        };
        let result = resolvable(TokenStream::new(), input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not &mut"));
    }
}
