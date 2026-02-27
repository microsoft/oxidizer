// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{FnArg, ItemFn, Pat, ReturnType, Token, Type, parse2};

/// Parsed arguments for the `thunk` attribute macro.
pub struct ThunkArgs {
    /// The path expression for the provider (specified via `from = ...`).
    pub provider_path: syn::Expr,
}

impl core::fmt::Debug for ThunkArgs {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ThunkArgs").finish_non_exhaustive()
    }
}

impl Parse for ThunkArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut provider_path: Option<syn::Expr> = None;
        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            if ident == "from" {
                provider_path = Some(input.parse()?);
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            provider_path: provider_path.ok_or_else(|| input.error("Must specify 'from' source"))?,
        })
    }
}

/// Processes the `thunk` attribute macro.
///
/// # Errors
///
/// Returns an error if the macro input is invalid.
#[expect(clippy::too_many_lines, reason = "macro codegen is inherently verbose")]
pub fn thunk_impl(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let input_fn: ItemFn = parse2(input)?;
    let args: ThunkArgs = parse2(args)?;

    let fn_name = &input_fn.sig.ident;
    let inner_fn_name = format_ident!("__{}_inner", fn_name);
    let raw_struct_name = format_ident!("__{fn_name}_RawTask");
    let provider = &args.provider_path;
    let vis = &input_fn.vis;

    let ret_type = match &input_fn.sig.output {
        ReturnType::Default => quote! { () },
        ReturnType::Type(_, ty) => quote! { #ty },
    };

    let mut raw_fields = Vec::new();
    let mut raw_init = Vec::new();
    let mut safe_unwrap = Vec::new();
    let mut call_args = Vec::new();

    let has_receiver = matches!(input_fn.sig.inputs.first(), Some(FnArg::Receiver(_)));
    let is_mut_self = if let Some(FnArg::Receiver(recv)) = input_fn.sig.inputs.first() {
        recv.mutability.is_some()
    } else {
        false
    };

    let provider_str = quote!(#provider).to_string();
    for arg in &input_fn.sig.inputs {
        if let FnArg::Typed(pat_type) = arg
            && let Pat::Ident(pat_ident) = &*pat_type.pat
        {
            let name = &pat_ident.ident;
            let ty = &pat_type.ty;

            // Skip fields that are accessed via self (e.g. self.thunker).
            // Parameters that ARE the provider (exact name match) are still
            // packed because the inner function needs them as arguments.
            if provider_str.ends_with(&format!(".{name}")) {
                continue;
            }

            call_args.push(quote! { #name });
            if let Type::Reference(ref_ty) = &**ty {
                let inner_ty = &ref_ty.elem;
                if ref_ty.mutability.is_some() {
                    raw_fields.push(quote! { #name: *mut #inner_ty });
                    raw_init.push(quote! { #name: #name as *mut #inner_ty });
                    safe_unwrap.push(quote! {
                        // SAFETY: Pointer valid for the lifetime of the StackState.
                        let #name = unsafe { &mut *task.#name };
                    });
                } else {
                    raw_fields.push(quote! { #name: *const #inner_ty });
                    raw_init.push(quote! { #name: #name as *const #inner_ty });
                    safe_unwrap.push(quote! {
                        // SAFETY: Pointer valid for the lifetime of the StackState.
                        let #name = unsafe { &*task.#name };
                    });
                }
            } else {
                raw_fields.push(quote! { #name: #ty });
                raw_init.push(quote! { #name });
                safe_unwrap.push(quote! { let #name = task.#name; });
            }
        }
    }

    // The inner function is a sync copy of the original, emitted as a sibling.
    // When there is a receiver, it stays as a method (so `self` works naturally).
    // When there is no receiver, it stays as an associated/free function.
    let mut inner_fn = input_fn.clone();
    inner_fn.sig.ident = inner_fn_name.clone();
    inner_fn.sig.asyncness = None;
    inner_fn.vis = syn::Visibility::Inherited;
    inner_fn
        .attrs
        .retain(|attr| attr.path().is_ident("expect") || attr.path().is_ident("allow"));

    // Build the self-pointer field, initializer, shim reconstruction, and call
    // expression depending on whether there is a receiver.
    let (self_ptr_field, self_ptr_init, shim_self_ref, inner_call) = if has_receiver {
        let cast = if is_mut_self {
            quote! { &mut *task.self_ptr.cast::<Self>().cast_mut() }
        } else {
            quote! { &*task.self_ptr.cast::<Self>() }
        };
        let init = if is_mut_self {
            quote! { self_ptr: self as *mut Self as *const (), }
        } else {
            quote! { self_ptr: self as *const Self as *const (), }
        };
        (
            Some(quote! { self_ptr: *const (), }),
            Some(init),
            Some(quote! {
                // SAFETY: self_ptr is valid for the lifetime of the StackState.
                let self_ref = unsafe { #cast };
            }),
            quote! { self_ref.#inner_fn_name(#(#call_args),*) },
        )
    } else {
        (None, None, None, quote! { Self::#inner_fn_name(#(#call_args),*) })
    };

    let fn_inputs = &input_fn.sig.inputs;

    // The shim is a sibling associated fn so it can reference `Self` for the
    // pointer cast. It is `#[doc(hidden)]` to keep the public API clean.
    let shim_name = format_ident!("__{fn_name}_shim");

    Ok(quote! {
        // Sync inner function — sibling method/associated fn so `self`/`Self` work.
        #[inline]
        #[doc(hidden)]
        #[allow(
            clippy::needless_pass_by_ref_mut,
            clippy::needless_pass_by_value,
            clippy::use_self,
            clippy::unused_self,
            unused_variables,
        )]
        #inner_fn

        // Shim — runs on the worker thread. Sibling so `Self` is available for cast.
        #[doc(hidden)]
        #[allow(clippy::use_self)]
        fn #shim_name(ptr: *mut ()) {
            // The struct must be repeated here so the shim can name it.
            #[allow(non_camel_case_types, dead_code)]
            struct #raw_struct_name {
                #self_ptr_field
                #(#raw_fields,)*
            }

            // SAFETY: ptr points to a valid StackState on the caller's stack.
            let state = unsafe { &*ptr.cast::<::sync_thunk::StackState::<#ret_type, #raw_struct_name>>() };
            // SAFETY: We are the sole consumer; the caller has set the task.
            let task = unsafe { state.take_task() }.expect("thunk task was set before dispatch");

            #shim_self_ref
            #(#safe_unwrap)*

            let panic_result = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                #inner_call
            }));

            match panic_result {
                Ok(result) => {
                    // SAFETY: Sole writer; poller reads only after ready flag is set.
                    unsafe { state.complete(result) };
                }
                Err(_) => {
                    state.mark_panicked();
                }
            }
            state.wake();
        }

        // Async wrapper — the only user-visible function.
        #vis async fn #fn_name(#fn_inputs) -> #ret_type {
            #[allow(non_camel_case_types, dead_code)]
            struct #raw_struct_name {
                #self_ptr_field
                #(#raw_fields,)*
            }
            // SAFETY: Pointers are valid for the lifetime of the StackState.
            unsafe impl Send for #raw_struct_name {}

            let state = ::sync_thunk::StackState::<#ret_type, #raw_struct_name>::new();
            let raw_task = #raw_struct_name {
                #self_ptr_init
                #(#raw_init,)*
            };
            // SAFETY: No concurrent access — work item not yet sent.
            unsafe { state.set_task(raw_task); }

            let work_item = ::sync_thunk::WorkItem::new(
                state.as_mut_ptr().cast::<()>(),
                Self::#shim_name,
            );
            #provider.send(work_item);
            ::sync_thunk::ThunkFuture::new(&state).await
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thunk_args_parse_from_field() {
        let tokens = quote! { from = self.thunker };
        let args: ThunkArgs = parse2(tokens).unwrap();
        let provider = &args.provider_path;
        let path_str = quote!(#provider).to_string();
        assert!(!path_str.is_empty());
    }

    #[test]
    fn thunk_args_parse_from_ident() {
        let tokens = quote! { from = thunker };
        let parsed: ThunkArgs = parse2(tokens).unwrap();
        let provider = &parsed.provider_path;
        let path_str = quote!(#provider).to_string();
        assert!(!path_str.is_empty());
    }

    #[test]
    fn thunk_args_debug() {
        let tokens = quote! { from = self.thunker };
        let parsed: ThunkArgs = parse2(tokens).unwrap();
        let debug = format!("{parsed:?}");
        assert!(debug.contains("ThunkArgs"));
    }

    #[test]
    fn thunk_args_missing_from() {
        let tokens = quote! {};
        let result: syn::Result<ThunkArgs> = parse2(tokens);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Must specify 'from' source"));
    }

    #[test]
    fn thunk_args_missing_equals() {
        let tokens = quote! { from };
        let result: syn::Result<ThunkArgs> = parse2(tokens);
        assert!(result.unwrap_err().to_string().contains("expected `=`"));
    }

    #[test]
    fn thunk_args_with_trailing_comma() {
        let tokens = quote! { from = self.thunker, };
        let parsed: ThunkArgs = parse2(tokens).unwrap();
        let provider = &parsed.provider_path;
        let path_str = quote!(#provider).to_string();
        assert!(!path_str.is_empty());
    }

    #[test]
    fn thunk_impl_ref_self_no_params() {
        let attr_args = quote! { from = self.thunker };
        let item = quote! {
            async fn work(&self) -> u64 {
                42
            }
        };
        let output = thunk_impl(attr_args, item).unwrap().to_string();
        assert!(output.contains("__work_inner"));
        assert!(output.contains("__work_shim"));
        assert!(output.contains("StackState"));
    }

    #[test]
    fn thunk_impl_mut_self() {
        let attr_args = quote! { from = self.thunker };
        let item = quote! {
            async fn work(&mut self) -> u64 { 42 }
        };
        let output = thunk_impl(attr_args, item).unwrap().to_string();
        assert!(output.contains("cast_mut"));
    }

    #[test]
    fn thunk_impl_no_receiver() {
        let attr_args = quote! { from = thunker };
        let item = quote! {
            async fn create(thunker: &Thunker, name: String) -> Self {
                Self { name }
            }
        };
        let output = thunk_impl(attr_args, item).unwrap().to_string();
        assert!(!output.contains("self_ptr"));
    }

    #[test]
    fn thunk_impl_unit_return() {
        let attr_args = quote! { from = self.thunker };
        let item = quote! {
            async fn fire(&self) {}
        };
        thunk_impl(attr_args, item).unwrap();
    }

    #[test]
    fn thunk_impl_not_a_function() {
        let attr_args = quote! { from = self.thunker };
        let item = quote! {
            struct Foo;
        };
        thunk_impl(attr_args, item).unwrap_err();
    }

    #[test]
    fn thunk_impl_ref_and_mut_ref_params() {
        let attr_args = quote! { from = self.thunker };
        let item = quote! {
            async fn work(&self, a: &str, b: &mut Vec<u8>) -> usize { 0 }
        };
        let output = thunk_impl(attr_args, item).unwrap().to_string();
        assert!(output.contains("* const str"));
        assert!(output.contains("* mut Vec"));
    }

    #[test]
    fn thunk_impl_owned_params() {
        let attr_args = quote! { from = self.thunker };
        let item = quote! {
            async fn work(&self, data: Vec<u8>) -> usize { data.len() }
        };
        thunk_impl(attr_args, item).unwrap();
    }
}
