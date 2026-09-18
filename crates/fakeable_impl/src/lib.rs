// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation details for the `fakeable` procedural macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemImpl, ItemStruct, parse_quote};

mod args;

use args::FakeableArgs;

/// Helper function to generate the fakes feature cfg attribute
fn fakes_cfg_attr(fakes_attribute: &str) -> proc_macro2::TokenStream {
    quote! { #[cfg(any(feature = #fakes_attribute, test))] }
}

/// Helper function to generate a method call with optional await
fn generate_method_call(
    target: &proc_macro2::Ident,
    method_name: &proc_macro2::Ident,
    param_names: &[proc_macro2::Ident],
    is_async: bool,
) -> proc_macro2::TokenStream {
    let await_suffix = is_async.then(|| quote! { .await });
    quote! { #target.#method_name(#(#param_names),*) #await_suffix }
}

#[must_use]
/// Expands a parsed `fakeable` attribute invocation.
///
/// Parse failures are returned as `compile_error!` tokens.
pub fn fakeable_impl(args: TokenStream, input: TokenStream) -> TokenStream {
    let args: FakeableArgs = match syn::parse2(args) {
        Ok(args) => args,
        Err(error) => return error.into_compile_error(),
    };

    // Try to parse as struct first, then as impl
    if let Ok(item_struct) = syn::parse2::<ItemStruct>(input.clone()) {
        process_struct(&args, &item_struct)
    } else if let Ok(item_impl) = syn::parse2::<ItemImpl>(input.clone()) {
        process_impl(&args, &item_impl)
    } else {
        syn::Error::new_spanned(input, "fakeable attribute can only be applied to structs or impl blocks").into_compile_error()
    }
}

/// Process struct definitions
fn process_struct(args: &FakeableArgs, item_struct: &ItemStruct) -> proc_macro2::TokenStream {
    let struct_name = &item_struct.ident;
    let struct_vis = &item_struct.vis;
    let struct_generics = &item_struct.generics;
    let struct_attrs = &item_struct.attrs;

    // Generate internal type names without prefix, to be placed in dedicated module
    let enum_name = quote::format_ident!("Enum");
    let helper_module_name = quote::format_ident!("__fakeable__{}", struct_name);

    // Extract the fake implementation path if provided
    let fake_impl_path = match args.fake_impl.as_ref() {
        Some(path) => path.clone(),
        None => {
            return syn::Error::new_spanned(struct_name, "fake_impl must be specified for struct definitions").into_compile_error();
        }
    };

    // Extract the fake constructor name (default to "fake" if not provided)
    let fake_constructor_name = args.fake_constructor.as_deref().unwrap_or("fake").to_string();
    let fake_constructor_ident = quote::format_ident!("{}", fake_constructor_name);

    // Parse the fakes attribute
    let fakes_attribute = &args.fakes_feature;
    let fakes_cfg = fakes_cfg_attr(fakes_attribute);

    // Extract derive attributes to apply to internal types
    let (derive_attrs, _other_attrs): (Vec<_>, Vec<_>) = struct_attrs
        .iter()
        .cloned()
        .partition(|attr| attr.path().get_ident().is_some_and(|ident| ident == "derive"));

    let transformed_struct_attrs: Vec<syn::Attribute> = struct_attrs.iter().map(transform_expect_to_allow).collect();

    let mut real_struct = item_struct.clone();
    real_struct.vis = parse_quote!(pub(super));

    // Make all fields pub(super) so they can be accessed from the impl blocks
    match &mut real_struct.fields {
        syn::Fields::Named(fields_named) => {
            for field in &mut fields_named.named {
                field.vis = parse_quote!(pub(super));
            }
        }
        syn::Fields::Unnamed(fields_unnamed) => {
            for field in &mut fields_unnamed.unnamed {
                field.vis = parse_quote!(pub(super));
            }
        }
        syn::Fields::Unit => {}
    }

    // Compile-time guard against an oversized inline fake. The wrapper enum is sized
    // for its largest variant, so a big, non-pointer fake bloats every instance of the
    // real type. `size_of` needs concrete types, so the guard is skipped for generics.
    let size_assert = if struct_generics.params.is_empty() {
        quote! {
            #fakes_cfg
            const _: () = {
                let fake_size = ::core::mem::size_of::<#fake_impl_path>();
                let real_size = ::core::mem::size_of::<#struct_name>();
                assert!(
                    !(fake_size > 256 && fake_size >= real_size.saturating_mul(2)),
                    concat!(
                        "fakeable: the fake implementation for `",
                        stringify!(#struct_name),
                        "` is larger than 256 bytes and at least 2x the real type, which bloats the wrapper. ",
                        "Wrap the fake behind a pointer instead, e.g. `fake_impl = std::sync::Arc<...>`."
                    )
                );
            };
        }
    } else {
        quote! {}
    };

    quote! {
        // Helper module containing internal types
        #[allow(non_snake_case)]
        mod #helper_module_name {
            use super::*;

            // Internal enum - needs derive attributes for wrapper struct to work
            #(#derive_attrs)*
            pub(super) enum #enum_name #struct_generics {
                Real(#struct_name #struct_generics),
                #fakes_cfg
                Fake(#fake_impl_path),
            }

            // Real struct (copy of the original struct but with a different name)
            #real_struct

            // Compile-time size guard for the fake implementation.
            #size_assert
        }

        // Wrapper struct
        #(#transformed_struct_attrs)*
        #struct_vis struct #struct_name #struct_generics {
            inner: #helper_module_name::#enum_name #struct_generics,
        }

        // Basic implementation for the wrapper struct
        impl #struct_generics #struct_name #struct_generics {
            #fakes_cfg
            #struct_vis fn #fake_constructor_ident(fake_impl: #fake_impl_path) -> Self {
                Self {
                    inner: #helper_module_name::#enum_name::Fake(fake_impl),
                }
            }
        }
    }
}

/// Process impl blocks
fn process_impl(args: &FakeableArgs, item_impl: &ItemImpl) -> proc_macro2::TokenStream {
    // Extract the struct name from the impl target
    let struct_name = match extract_struct_name(item_impl) {
        Ok(name) => name,
        Err(err) => return err.into_compile_error(),
    };

    // Generate internal type names without prefix, to be placed in dedicated module
    let enum_name = syn::Ident::new("Enum", proc_macro2::Span::call_site());
    let helper_module_name = quote::format_ident!("__fakeable__{struct_name}");
    let fakes_attribute = &args.fakes_feature;

    let mut real_impl = item_impl.clone();
    *real_impl.self_ty = parse_quote!(#helper_module_name::#struct_name);

    let wrapper_impl = match generate_wrapper_impl(item_impl, &struct_name, &enum_name, &helper_module_name, fakes_attribute) {
        Ok(impl_block) => impl_block,
        Err(err) => return err.into_compile_error(),
    };

    // Generate mockall fake if requested
    let mockall_fake = if args.generate_mockall_fake == Some(true) {
        let fake_name = struct_name.to_string();
        let fake_module = args.mockall_fake_module.as_deref().unwrap_or("fakes");
        generate_mockall_fake(item_impl, &fake_name, fakes_attribute, fake_module)
    } else {
        quote! {}
    };

    quote! {
        #real_impl

        #wrapper_impl

        #mockall_fake
    }
}

/// Extracts the struct name from an impl block.
fn extract_struct_name(item_impl: &ItemImpl) -> Result<proc_macro2::Ident, syn::Error> {
    match &*item_impl.self_ty {
        syn::Type::Path(type_path) => {
            if let Some(segment) = type_path.path.segments.last() {
                Ok(segment.ident.clone())
            } else {
                Err(syn::Error::new_spanned(
                    &item_impl.self_ty,
                    "unable to extract struct name from impl target",
                ))
            }
        }
        _ => Err(syn::Error::new_spanned(&item_impl.self_ty, "impl target must be a simple path")),
    }
}

/// Generates an impl block for the wrapper struct that delegates to the internal enum.
fn generate_wrapper_impl(
    original_impl: &ItemImpl,
    struct_name: &proc_macro2::Ident,
    enum_name: &proc_macro2::Ident,
    helper_module_name: &proc_macro2::Ident,
    fakes_attribute: &str,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    let trait_path = &original_impl.trait_;

    // For trait impls, we need to implement all methods regardless of visibility
    let is_trait_impl = trait_path.is_some();

    let mut delegation_methods: Vec<syn::ImplItem> = Vec::new();

    for item in &original_impl.items {
        if let syn::ImplItem::Fn(method) = item {
            // For trait impls, include all methods (they typically have Inherited visibility)
            // For regular impls, only include public, pub(crate), and pub(super) methods
            // Skip private methods to allow unsupported patterns and methods without self parameter
            if !is_trait_impl {
                let is_public_or_restricted = matches!(method.vis, syn::Visibility::Public(_) | syn::Visibility::Restricted(_));
                if !is_public_or_restricted {
                    continue;
                }
            }

            let delegation_method = generate_delegation_method(method, fakes_attribute, enum_name, struct_name, helper_module_name)?;
            delegation_methods.push(syn::ImplItem::Fn(delegation_method));
        }
    }

    let mut fake_impl = original_impl.clone();
    fake_impl.items = delegation_methods;
    Ok(quote! { #fake_impl })
}

/// Generates a delegation method that matches on the internal enum and calls the appropriate implementation.
fn generate_delegation_method(
    original_method: &syn::ImplItemFn,
    fakes_attribute: &str,
    enum_name: &proc_macro2::Ident,
    real_struct_name: &proc_macro2::Ident,
    helper_module_name: &proc_macro2::Ident,
) -> Result<syn::ImplItemFn, syn::Error> {
    let method_vis = &original_method.vis;
    let method_sig = &original_method.sig;
    let method_name = &method_sig.ident;
    let is_async = method_sig.asyncness.is_some();
    let receiver = method_sig.receiver();

    // Extract parameter names and determine if this is a constructor or returns Self
    let method_info = extract_method_info(method_sig)?;
    let fakes_cfg = fakes_cfg_attr(fakes_attribute);

    let method_body = if method_info.is_constructor {
        generate_constructor_body(
            enum_name,
            real_struct_name,
            helper_module_name,
            method_name,
            &method_info.param_names,
            is_async,
        )
    } else if let Some(receiver) = receiver {
        if method_info.returns_self_with_receiver {
            generate_method_with_self_return_body(
                enum_name,
                helper_module_name,
                method_name,
                &method_info.param_names,
                is_async,
                &fakes_cfg,
                receiver,
            )
        } else {
            generate_method_body(
                enum_name,
                helper_module_name,
                method_name,
                &method_info.param_names,
                is_async,
                &fakes_cfg,
                receiver,
            )
        }
    } else {
        return Err(syn::Error::new_spanned(
            method_sig,
            "methods without self parameter are not supported except for constructors",
        ));
    };

    // Transform expect attributes to allow attributes
    let method_attrs: Vec<syn::Attribute> = original_method.attrs.iter().map(transform_expect_to_allow).collect();

    Ok(parse_quote! {
        #(#method_attrs)*
        #[allow(unused_mut)]
        #[allow(clippy::used_underscore_binding)]
        #method_vis #method_sig {
            #method_body
        }
    })
}

/// Generates the body for constructor methods
fn generate_constructor_body(
    enum_name: &proc_macro2::Ident,
    real_struct_name: &proc_macro2::Ident,
    helper_module_name: &proc_macro2::Ident,
    method_name: &proc_macro2::Ident,
    param_names: &[proc_macro2::Ident],
    is_async: bool,
) -> proc_macro2::TokenStream {
    let await_suffix = is_async.then(|| quote! { .await });
    let method_call = quote! {
        #helper_module_name::#real_struct_name::#method_name(#(#param_names),*)#await_suffix
    };

    quote! {
        Self {
            inner: #helper_module_name::#enum_name::Real(#method_call),
        }
    }
}

/// Generates the body for immutable methods
fn generate_method_body(
    enum_name: &proc_macro2::Ident,
    helper_module_name: &proc_macro2::Ident,
    method_name: &proc_macro2::Ident,
    param_names: &[proc_macro2::Ident],
    is_async: bool,
    fakes_cfg: &proc_macro2::TokenStream,
    receiver: &syn::Receiver,
) -> proc_macro2::TokenStream {
    let real_call = generate_method_call(
        &proc_macro2::Ident::new("real", proc_macro2::Span::call_site()),
        method_name,
        param_names,
        is_async,
    );
    let fake_call = generate_method_call(
        &proc_macro2::Ident::new("fake", proc_macro2::Span::call_site()),
        method_name,
        param_names,
        is_async,
    );

    quote! {
        match #receiver .inner {
            #helper_module_name::#enum_name::Real(real) => #real_call,
            #fakes_cfg
            #helper_module_name::#enum_name::Fake(fake) => #fake_call,
        }
    }
}

/// Generates the body for methods with receivers that return Self.
/// This is a hybrid approach: it matches on the inner enum (like regular methods)
/// but wraps the returned Self value (like constructors).
fn generate_method_with_self_return_body(
    enum_name: &proc_macro2::Ident,
    helper_module_name: &proc_macro2::Ident,
    method_name: &proc_macro2::Ident,
    param_names: &[proc_macro2::Ident],
    is_async: bool,
    fakes_cfg: &proc_macro2::TokenStream,
    receiver: &syn::Receiver,
) -> proc_macro2::TokenStream {
    let real_call = generate_method_call(
        &proc_macro2::Ident::new("real", proc_macro2::Span::call_site()),
        method_name,
        param_names,
        is_async,
    );
    let fake_call = generate_method_call(
        &proc_macro2::Ident::new("fake", proc_macro2::Span::call_site()),
        method_name,
        param_names,
        is_async,
    );

    quote! {
        match #receiver .inner {
            #helper_module_name::#enum_name::Real(real) => {
                Self {
                    inner: #helper_module_name::#enum_name::Real(#real_call),
                }
            }
            #fakes_cfg
            #helper_module_name::#enum_name::Fake(fake) => {
                Self {
                    inner: #helper_module_name::#enum_name::Fake(#fake_call),
                }
            }
        }
    }
}

/// Information extracted from a method signature for code generation.
struct MethodInfo {
    /// The names of the method's non-self parameters.
    param_names: Vec<proc_macro2::Ident>,
    /// Whether this method is a constructor (no self receiver, returns Self).
    is_constructor: bool,
    /// Whether this method has a receiver and returns Self (needs wrapping).
    returns_self_with_receiver: bool,
}

/// Extracts parameter names from a method signature and determines if it's a constructor.
fn extract_method_info(sig: &syn::Signature) -> Result<MethodInfo, syn::Error> {
    let mut param_names = Vec::new();
    let mut has_self = false;

    for input in &sig.inputs {
        match input {
            syn::FnArg::Receiver(_) => {
                has_self = true;
            }
            syn::FnArg::Typed(syn::PatType { pat, .. }) => {
                if let syn::Pat::Ident(pat_ident) = &**pat {
                    // Extract just the identifier name, ignoring mut keyword
                    param_names.push(pat_ident.ident.clone());
                } else {
                    return Err(syn::Error::new_spanned(pat, "complex parameter patterns are not supported"));
                }
            }
        }
    }

    let returns_self_flag = returns_self(&sig.output);

    // A method is considered a constructor if it doesn't have &self and returns Self
    let is_constructor = !has_self && returns_self_flag;

    // Wrap the result in Self { inner: ... } for methods with receivers that return Self
    let returns_self_with_receiver = has_self && returns_self_flag;

    Ok(MethodInfo {
        param_names,
        is_constructor,
        returns_self_with_receiver,
    })
}

/// Checks if a function returns Self.
fn returns_self(output: &syn::ReturnType) -> bool {
    match output {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => {
            if let syn::Type::Path(type_path) = &**ty {
                if let Some(segment) = type_path.path.segments.last() {
                    segment.ident == "Self"
                } else {
                    false
                }
            } else {
                false
            }
        }
    }
}

/// Converts an async function signature to a non-async function with impl Future return type because
/// for methods with impl Future return types, mockall supports futures to be returned by mocks, unlike
/// for async methods, for which it only supports returning values that are wrapped in immediate futures.
fn convert_async_to_impl_future(sig: &syn::Signature) -> proc_macro2::TokenStream {
    let ident = &sig.ident;
    let generics = &sig.generics;
    let inputs = &sig.inputs;

    // Extract the original return type
    let output_type = match &sig.output {
        syn::ReturnType::Default => quote! { () },
        syn::ReturnType::Type(_, ty) => quote! { #ty },
    };

    // Create the new signature without async but with impl Future return type
    quote! {
        fn #ident #generics(#inputs) -> impl std::future::Future<Output = #output_type> + Send
    }
}

/// Adds explicit lifetimes to a method signature for mockall compatibility.
/// This is needed because mockall cannot handle elided lifetimes in generic type arguments.
fn add_explicit_lifetimes(sig: &syn::Signature) -> syn::Signature {
    let mut sig = sig.clone();

    // Check if the signature already has explicit lifetime parameters
    let has_explicit_lifetimes = sig
        .generics
        .params
        .iter()
        .any(|param| matches!(param, syn::GenericParam::Lifetime(_)));

    // If there are already explicit lifetimes, return as-is
    if has_explicit_lifetimes {
        return sig;
    }

    // Check if any input types contain references that might need explicit lifetimes
    let needs_lifetime = sig.inputs.iter().any(|input| {
        if let syn::FnArg::Typed(syn::PatType { ty, .. }) = input {
            contains_reference_in_generic(ty)
        } else {
            false
        }
    });

    // If no references in generics are found, return as-is
    if !needs_lifetime {
        return sig;
    }

    // Add a lifetime parameter 'mock
    let lifetime: syn::LifetimeParam = syn::parse_quote!('mock);
    sig.generics.params.insert(0, syn::GenericParam::Lifetime(lifetime));

    // Add the lifetime to all elided references in generic type arguments
    for input in &mut sig.inputs {
        if let syn::FnArg::Typed(syn::PatType { ty, .. }) = input {
            add_lifetime_to_references(ty, &syn::parse_quote!('mock));
        }
    }

    sig
}

/// Checks if a type contains references within generic arguments.
fn contains_reference_in_generic(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(type_path) => {
            for segment in &type_path.path.segments {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(inner_ty) = arg
                            && (matches!(inner_ty, syn::Type::Reference(_)) || contains_reference_in_generic(inner_ty))
                        {
                            return true;
                        }
                    }
                }
            }
            false
        }
        // Direct references are fine
        syn::Type::Reference(_) | _ => false,
    }
}

/// Adds a lifetime to all elided references within generic type arguments.
fn add_lifetime_to_references(ty: &mut syn::Type, lifetime: &syn::Lifetime) {
    match ty {
        syn::Type::Path(type_path) => {
            for segment in &mut type_path.path.segments {
                if let syn::PathArguments::AngleBracketed(args) = &mut segment.arguments {
                    for arg in &mut args.args {
                        if let syn::GenericArgument::Type(inner_ty) = arg {
                            if let syn::Type::Reference(ref_ty) = inner_ty {
                                // Only add lifetime if it's elided
                                if ref_ty.lifetime.is_none() {
                                    ref_ty.lifetime = Some(lifetime.clone());
                                }
                            } else {
                                // Recursively process nested generics
                                add_lifetime_to_references(inner_ty, lifetime);
                            }
                        }
                    }
                }
            }
        }
        syn::Type::Reference(ref_ty) => {
            // Process the inner type recursively
            add_lifetime_to_references(&mut ref_ty.elem, lifetime);
        }
        _ => {}
    }
}

/// Generates a mockall mock! macro for the given impl block.
fn generate_mockall_fake(item_impl: &ItemImpl, fake_name: &str, fakes_attribute: &str, fake_module: &str) -> proc_macro2::TokenStream {
    // Use the provided name directly - mockall will add "Mock" prefix automatically
    let fake_ident = quote::format_ident!("{}", fake_name);

    // Extract methods from the impl block
    let mut mock_methods = Vec::new();

    for item in &item_impl.items {
        if let syn::ImplItem::Fn(method) = item {
            // Skip non-public methods
            let is_public = matches!(method.vis, syn::Visibility::Public(_));

            // Skip constructors (methods that don't take self and return Self)
            let has_self = method.sig.inputs.iter().any(|input| matches!(input, syn::FnArg::Receiver(_)));

            // Skip mutable methods as mockall doesn't support them well
            let is_mut = method
                .sig
                .inputs
                .iter()
                .any(|input| matches!(input, syn::FnArg::Receiver(receiver) if receiver.mutability.is_some()));

            if is_public && has_self && !is_mut {
                // Add explicit lifetimes to avoid mockall compilation errors
                let sig_with_lifetimes = add_explicit_lifetimes(&method.sig);

                // Convert async functions to use impl Future return type for better mockall compatibility
                let method_signature = if method.sig.asyncness.is_some() {
                    convert_async_to_impl_future(&sig_with_lifetimes)
                } else {
                    quote! { #sig_with_lifetimes }
                };
                mock_methods.push(quote! { pub #method_signature; });
            }
        }
    }

    let fakes_cfg = fakes_cfg_attr(fakes_attribute);

    if fake_module == "." {
        // Generate in current module
        quote! {
            #fakes_cfg
            mockall::mock! {
                #[derive(Debug)]
                pub #fake_ident {
                    #(#mock_methods)*
                }
            }
        }
    } else {
        // Generate in specified module
        let module_ident = quote::format_ident!("{}", fake_module);
        quote! {
            #fakes_cfg
            #[allow(clippy::all)]
            #[allow(clippy::pedantic)]
            #[allow(clippy::style)]
            pub mod #module_ident {
                use super::*;
                mockall::mock! {
                    #[derive(Debug)]
                    pub #fake_ident {
                        #(#mock_methods)*
                    }
                }
            }
        }
    }
}

/// Wrapper code might not have the same lint expectations as the original code, so we need to
/// transform any expect attributes to allow attributes to avoid lint errors in the generated code.
///
/// Handles both `#[expect(...)]` and `#[cfg_attr(something, expect(...))]`
fn transform_expect_to_allow(attr: &syn::Attribute) -> syn::Attribute {
    use syn::Meta;

    let mut attr = attr.clone();

    // Case 1: #[expect(...)] -> #[allow(...)]
    if attr.path().is_ident("expect") {
        if let Meta::List(meta_list) = &mut attr.meta {
            meta_list.path = parse_quote!(allow);
        }
        return attr;
    }

    // Case 2: #[cfg_attr(...)]
    if attr.path().is_ident("cfg_attr")
        && let Meta::List(meta_list) = &mut attr.meta
    {
        let tokens = &meta_list.tokens;
        let new_tokens = rewrite_expect_in_tokens(tokens.clone());
        meta_list.tokens = new_tokens;
        return attr;
    }

    attr
}

/// Rewrite occurrences of `expect(...)` → `allow(...)` inside a token stream
fn rewrite_expect_in_tokens(tokens: TokenStream) -> TokenStream {
    use proc_macro2::TokenTree;

    let mut output = TokenStream::new();
    let mut iter = tokens.into_iter().peekable();

    while let Some(tt) = iter.next() {
        match &tt {
            TokenTree::Ident(ident) if ident == "expect" => {
                // Look ahead: expect ( ... )
                if let Some(TokenTree::Group(group)) = iter.peek()
                    && group.delimiter() == proc_macro2::Delimiter::Parenthesis
                {
                    let Some(TokenTree::Group(group)) = iter.next() else {
                        unreachable!()
                    };

                    // rewrite: expect(...) → allow(...)
                    let new_ident = syn::Ident::new("allow", ident.span());
                    let rewritten_group =
                        proc_macro2::Group::new(proc_macro2::Delimiter::Parenthesis, rewrite_expect_in_tokens(group.stream()));

                    output.extend([TokenTree::Ident(new_ident), TokenTree::Group(rewritten_group)]);
                    continue;
                }

                // Plain "expect" not followed by "(...)"
                output.extend([tt]);
            }

            TokenTree::Group(group) => {
                let new_group = proc_macro2::Group::new(group.delimiter(), rewrite_expect_in_tokens(group.stream()));
                output.extend([TokenTree::Group(new_group)]);
            }

            _ => output.extend([tt]),
        }
    }

    output
}
