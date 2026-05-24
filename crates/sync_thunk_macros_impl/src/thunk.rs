// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::visit_mut::{self, VisitMut};
use syn::{FnArg, Ident, ItemFn, Pat, ReturnType, Token, Type, TypePath, parse2};

/// Detects whether a type contains any reference to `Self`. Used to decide
/// when to lift the arg type into a fresh generic parameter on the local
/// `__RawTask` struct (the struct is a nested item and cannot see the
/// enclosing impl's `Self`, and adding a single carrier generic isn't
/// enough for shapes like `Self::Output` that would require trait bounds
/// the macro can't know).
struct ContainsSelf {
    found: bool,
}

impl VisitMut for ContainsSelf {
    fn visit_type_path_mut(&mut self, i: &mut TypePath) {
        if !i.path.segments.is_empty() && i.path.leading_colon.is_none() && i.path.segments[0].ident == "Self" {
            self.found = true;
        }
        visit_mut::visit_type_path_mut(self, i);
    }
}

fn ty_contains_self(ty: &Type) -> bool {
    let mut v = ContainsSelf { found: false };
    let mut ty = ty.clone();
    v.visit_type_mut(&mut ty);
    v.found
}

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
    // Extra generic parameters added to `__RawTask` for any arg type that
    // mentions `Self`. The struct cannot itself reference the enclosing
    // impl's `Self` (nested items don't inherit it), and adding a single
    // `__ThunkImplSelf` carrier doesn't help for shapes like `Self::Output`
    // or `<Self as Trait>::Bar` (which would require trait bounds on the
    // carrier that the macro can't know). Instead, we lift each such arg
    // type into its own generic parameter and let the wrapper / shim — both
    // of which DO have access to `Self` — supply the concrete type at the
    // instantiation site.
    let mut self_ty_generics: Vec<Ident> = Vec::new();
    let mut self_ty_concrete: Vec<TokenStream> = Vec::new();
    // Compile-time `T: Send` / `T: Sync` checks for everything we ship across
    // threads. The generated `unsafe impl Send for __RawTask {}` is
    // unconditional in T, so without these the user could smuggle non-Send /
    // non-Sync values onto the worker.
    let mut send_sync_asserts = Vec::new();

    if let Some(FnArg::Receiver(recv)) = input_fn.sig.inputs.first() {
        // Receivers — in any shape — would force us to either (a) capture the
        // receiver by raw pointer into the wrapper's frame (which, combined
        // with `mem::forget` on the future, lets safe code drop the referent
        // while the worker is still using it = use-after-free) or (b) move
        // the receiver into the work item (which `self`-by-value would
        // require but is incompatible with the `&Self`-derived inner-fn
        // call site). Rather than pick a half-sound model, reject all
        // receivers and require the user to pass `Arc<Self>` (or another
        // owned `Send + 'static` shape) as a regular parameter — exactly
        // mirroring `tokio::task::spawn_blocking`'s closure-captures-by-value
        // call site.
        return Err(syn::Error::new_spanned(
            recv,
            "#[thunk] does not support `self`, `&self`, `&mut self`, or typed `self` receivers. \
             Borrowed receivers allow safe code to trigger use-after-free via `mem::forget`; \
             owned receivers force a call-site shape the macro cannot generate soundly. \
             Rewrite the method as a free associated function taking `Arc<Self>` (or another \
             owned `Send + 'static` value) as a parameter:\n\n    \
             #[thunk(from = me.thunker)]\n    \
             async fn work(me: Arc<Self>, ...) -> R { ... }",
        ));
    }

    for arg in &input_fn.sig.inputs {
        if let FnArg::Typed(pat_type) = arg
            && let Pat::Ident(pat_ident) = &*pat_type.pat
        {
            let name = &pat_ident.ident;
            let ty = &pat_type.ty;

            call_args.push(quote! { #name });
            if let Type::Reference(ref_ty) = &**ty {
                // Reference parameters need very specific lifetime guarantees
                // to avoid use-after-free under `mem::forget(future)`. The
                // borrow checker tracks reference lifetimes through the
                // wrapper future's type, but `mem::forget` releases the borrow
                // under NLL while the worker may still be executing — letting
                // safe code drop the referent and triggering UAF.
                //
                // The one exception is `&'static T` (and `&'static mut T`):
                // the referent lives for the entire program, so no caller
                // action can ever invalidate the pointer. `assert_send_static`
                // below additionally requires `&'static T: Send`, which
                // implies `T: Sync` — so the worker's read is also sound.
                let is_static = ref_ty.lifetime.as_ref().is_some_and(|lt| lt.ident == "static");
                if !is_static {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "#[thunk] only accepts `&'static T` / `&'static mut T` reference \
                         parameters. Any shorter-lifetime reference can be invalidated by \
                         `mem::forget`-ing the wrapper future and dropping the referent while \
                         the worker is still using it (= use-after-free). For non-`'static` \
                         data, pass an owned value instead (e.g. `T`, `Arc<T>`, `Box<T>`, or a \
                         clone).",
                    ));
                }
            }
            raw_fields.push({
                if ty_contains_self(ty) {
                    let g = format_ident!("__SelfTy{}", self_ty_generics.len());
                    self_ty_generics.push(g.clone());
                    self_ty_concrete.push(quote! { #ty });
                    quote! { #name: #g }
                } else {
                    quote! { #name: #ty }
                }
            });
            raw_init.push(quote! { #name });
            safe_unwrap.push(quote! { let #name = task.#name; });
            // Owned values are moved to the worker thread; `'static` is
            // required because the wrapper future may be `mem::forget`-ed,
            // so the value cannot transitively borrow non-`'static` state.
            send_sync_asserts.push(quote! {
                ::sync_thunk::__private::assert_send_static::<#ty>();
            });
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

    let fn_inputs = &input_fn.sig.inputs;

    // The shim is a sibling associated fn so it can reference `Self` for the
    // pointer cast. It is `#[doc(hidden)]` to keep the public API clean.
    let shim_name = format_ident!("__{fn_name}_shim");

    // Generic parameter list for `__RawTask`: `__ThunkImplSelf` carrier plus
    // one fresh `__SelfTyN` per arg type that mentions `Self`.
    let raw_generics_decl = quote! {
        __ThunkImplSelf: ?Sized #(, #self_ty_generics)*
    };
    let raw_generics_use = quote! {
        Self #(, #self_ty_concrete)*
    };
    // Marker generics used by the `unsafe impl Send` block (no concrete
    // bounds on the lifted types — `Send` is enforced separately by the
    // `assert_send_static` calls).
    let send_impl_generics = quote! {
        __ThunkImplSelf: ?Sized #(, #self_ty_generics)*
    };
    let send_impl_use = quote! {
        __ThunkImplSelf #(, #self_ty_generics)*
    };

    Ok(quote! {
        // Sync inner function — sibling associated fn so `Self` works.
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
            // The local `__RawTask` struct cannot literally reference the
            // surrounding impl's `Self` (nested items don't inherit `Self`).
            // Instead, we make it generic over a type parameter and supply
            // the concrete `Self` at every instantiation site, where the
            // alias `Self` is still in fn scope inside the impl.
            #[repr(C)]
            #[allow(non_camel_case_types, dead_code)]
            struct #raw_struct_name<#raw_generics_decl> {
                #(#raw_fields,)*
                _self_phantom: ::core::marker::PhantomData<fn() -> __ThunkImplSelf>,
            }

            // SAFETY: ptr points to a valid StackState on the caller's stack.
            let state = unsafe { &*ptr.cast::<::sync_thunk::StackState::<#ret_type, #raw_struct_name<#raw_generics_use>>>() };

            // RAII guard: `mark_worker_done()` must run unconditionally as the
            // very last operation on `state`, even if `wake()` (which invokes
            // user-supplied Waker code) panics. Without this, the caller's
            // Drop guard would spin forever.
            struct __DoneOnDrop<'a, R, T>(&'a ::sync_thunk::StackState<R, T>);
            impl<R, T> ::std::ops::Drop for __DoneOnDrop<'_, R, T> {
                fn drop(&mut self) {
                    self.0.mark_worker_done();
                }
            }
            let __done_guard = __DoneOnDrop(state);

            // Run the entire shim body inside `catch_unwind` so that even
            // exotic failures (e.g. a missing task slot) are converted into
            // a marked-panicked state instead of unwinding into the worker.
            let panic_result = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                // SAFETY: We are the sole consumer; the caller has set the task.
                let task = unsafe { state.take_task() }.expect("thunk task was set before dispatch");

                #(#safe_unwrap)*

                Self::#inner_fn_name(#(#call_args),*)
            }));

            match panic_result {
                Ok(result) => {
                    // SAFETY: Sole writer; poller reads only after ready flag is set.
                    unsafe { state.complete(result) };
                }
                Err(payload) => {
                    // SAFETY: Sole writer; poller reads only after ready flag is set.
                    unsafe { state.mark_panicked(payload) };
                }
            }
            state.wake();
            // `__done_guard` drops here, calling `mark_worker_done()`.
            ::core::mem::drop(__done_guard);
        }

        // Async wrapper — the only user-visible function.
        #vis async fn #fn_name(#fn_inputs) -> #ret_type {
            // `#[repr(C)]` pins field order so the wrapper- and shim-side
            // structs (which are nominally distinct) share a layout. The
            // `__ThunkImplSelf` type parameter lets us forward the impl's
            // `Self` through the local struct definition (see shim above).
            #[repr(C)]
            #[allow(non_camel_case_types, dead_code)]
            struct #raw_struct_name<#raw_generics_decl> {
                #(#raw_fields,)*
                _self_phantom: ::core::marker::PhantomData<fn() -> __ThunkImplSelf>,
            }
            // SAFETY: The assertions below ensure every payload type is
            // `Send + 'static`, so cross-thread access is sound. There are no
            // raw pointers into caller-side storage (all parameters are
            // owned), so there is no use-after-free risk on `mem::forget`.
            unsafe impl<#send_impl_generics> Send for #raw_struct_name<#send_impl_use> {}

            // Compile-time `Send + 'static` checks for every value shipped to
            // the worker thread. These are calls to empty `const fn`s whose
            // only purpose is to gate monomorphization on the appropriate
            // bounds. They compile away.
            #(#send_sync_asserts)*

            let state = ::sync_thunk::StackState::<#ret_type, #raw_struct_name<#raw_generics_use>>::new();

            // Drop guard: install IMMEDIATELY after `state` is created and
            // BEFORE any code that could panic (provider expression
            // evaluation, `clone_thunker`, raw-task assembly, `send`). If
            // anything between here and a successful `send` panics, the
            // guard releases `StackState`'s `Drop` spin-loop so the
            // caller's stack can unwind instead of hanging forever on a
            // worker that will never run. `mem::forget` below cancels the
            // guard once the work item is in the worker's hands.
            struct __AbandonOnPanic<'a, R, T>(&'a ::sync_thunk::StackState<R, T>);
            impl<R, T> ::std::ops::Drop for __AbandonOnPanic<'_, R, T> {
                fn drop(&mut self) {
                    self.0.abandon();
                }
            }
            let __guard = __AbandonOnPanic(&state);

            // Capture the sender BEFORE moving any params into `raw_task`.
            // The provider expression (e.g. `me.thunker`) typically borrows
            // through one of the parameters we're about to move into the
            // work item; evaluating it after the move would fail to compile.
            // We clone instead of borrow so `me` (or whichever param the
            // provider walks through) is free to move afterwards. `Thunker`
            // (and `&Thunker`, `Arc<Thunker>`, etc.) all resolve `.clone()`
            // to a cheap `Arc` bump, so this is essentially free.
            let __thunk_sender = ::sync_thunk::__private::clone_thunker(&#provider);

            let raw_task = #raw_struct_name::<#raw_generics_use> {
                #(#raw_init,)*
                _self_phantom: ::core::marker::PhantomData,
            };
            // SAFETY: No concurrent access — work item not yet sent.
            unsafe { state.set_task(raw_task); }

            let work_item = unsafe {
                // SAFETY: `state.as_mut_ptr()` is valid for the lifetime of
                // the `StackState` on this frame. The shim derives a
                // `&StackState` from the pointer and only performs operations
                // permitted by the documented `StackState` protocol. The
                // `__DoneOnDrop` guard ensures the shim does not unwind.
                ::sync_thunk::WorkItem::new(
                    state.as_mut_ptr().cast::<()>(),
                    Self::#shim_name,
                )
            };

            __thunk_sender.send(work_item);
            // Send succeeded — work item is now in the worker's hands and
            // the worker is responsible for calling `mark_worker_done`.
            ::std::mem::forget(__guard);

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
    fn thunk_impl_ref_self_rejected() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(&self) -> u64 { 42 }
        };
        let err = thunk_impl(attr_args, item).unwrap_err().to_string();
        assert!(err.contains("does not support `self`"), "got: {err}");
    }

    #[test]
    fn thunk_impl_owned_self_rejected() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(self) -> u64 { 42 }
        };
        let err = thunk_impl(attr_args, item).unwrap_err().to_string();
        assert!(err.contains("does not support `self`"), "got: {err}");
    }

    #[test]
    fn thunk_impl_mut_self_rejected() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(&mut self) -> u64 { 42 }
        };
        let err = thunk_impl(attr_args, item).unwrap_err().to_string();
        assert!(err.contains("does not support `self`"), "got: {err}");
    }

    #[test]
    fn thunk_impl_typed_self_rejected() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(self: Arc<Self>) -> u64 { 42 }
        };
        let err = thunk_impl(attr_args, item).unwrap_err().to_string();
        assert!(err.contains("does not support `self`"), "got: {err}");
    }

    #[test]
    fn thunk_impl_no_receiver() {
        let attr_args = quote! { from = thunker };
        let item = quote! {
            async fn create(thunker: ::std::sync::Arc<Thunker>, name: String) -> Self {
                Self { name }
            }
        };
        let output = thunk_impl(attr_args, item).unwrap().to_string();
        assert!(!output.contains("self_ptr"));
        assert!(output.contains("# [repr (C)]"));
    }

    #[test]
    fn thunk_impl_unit_return() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn fire(me: ::std::sync::Arc<Service>) {}
        };
        thunk_impl(attr_args, item).unwrap();
    }

    #[test]
    fn thunk_impl_not_a_function() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            struct Foo;
        };
        thunk_impl(attr_args, item).unwrap_err();
    }

    #[test]
    fn thunk_impl_mut_ref_param_rejected() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(me: Arc<Service>, b: &mut Vec<u8>) -> usize { 0 }
        };
        let err = thunk_impl(attr_args, item).unwrap_err().to_string();
        assert!(err.contains("only accepts `&'static T`"), "got: {err}");
    }

    #[test]
    fn thunk_impl_shared_ref_param_rejected() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(me: Arc<Service>, b: &Vec<u8>) -> usize { 0 }
        };
        let err = thunk_impl(attr_args, item).unwrap_err().to_string();
        assert!(err.contains("only accepts `&'static T`"), "got: {err}");
    }

    #[test]
    fn thunk_impl_static_ref_param_accepted() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(me: ::std::sync::Arc<Service>, s: &'static str) -> usize { s.len() }
        };
        thunk_impl(attr_args, item).unwrap();
    }

    #[test]
    fn thunk_impl_self_assoc_type_param_accepted() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(me: ::std::sync::Arc<Service>, x: Self::Output) -> u64 { 0 }
        };
        thunk_impl(attr_args, item).unwrap();
    }

    #[test]
    fn thunk_impl_owned_params() {
        let attr_args = quote! { from = me.thunker };
        let item = quote! {
            async fn work(me: ::std::sync::Arc<Service>, data: Vec<u8>) -> usize { data.len() }
        };
        thunk_impl(attr_args, item).unwrap();
    }
}
