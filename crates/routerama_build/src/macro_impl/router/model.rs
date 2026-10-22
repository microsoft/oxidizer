// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Plain data types describing the parsed `#[router]` impl.

use alloc::string::String;
use alloc::vec::Vec;

use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{Attribute, Type, WherePredicate};

use super::lowering::dispatch_label;
use crate::Route;
use crate::macro_impl::{RoutePredicates, RoutePriority, StaticHeader};

pub(super) struct Handler {
    pub(super) method: Ident,
    pub(super) variant: Ident,
    pub(super) kind: HandlerKind,
    pub(super) route_attrs: Vec<Attribute>,
    pub(super) predicates: RoutePredicates,
    pub(super) static_headers: Vec<StaticHeader>,
    pub(super) captures: Vec<(Ident, Type)>,
    pub(super) arguments: Vec<Argument>,
    pub(super) response_type: Type,
    pub(super) borrows_path: bool,
}

pub(super) struct DispatchArm {
    pub(super) variant: Ident,
    pub(super) route_attrs: Vec<Attribute>,
    pub(super) captures: Vec<(Ident, Type)>,
    pub(super) kind: DispatchKind,
}

pub(super) enum DispatchKind {
    Direct(usize),
    Overlap(Vec<usize>),
}

#[derive(Clone, Copy)]
pub(super) enum DispatchResponse {
    Concrete,
    Mounted,
    MountedHeterogeneous,
}

/// How a generated stage leaves the entry once it owns a response.
///
/// Stages return directly unless a router-wide `#[after]` needs a common
/// response epilogue.
#[derive(Clone, Copy)]
pub(super) enum DispatchExit {
    Return,
    Break,
}

/// The response shape and exit form shared by every stage of one entry.
#[derive(Clone, Copy)]
pub(super) struct DispatchBoundary {
    pub(super) response: DispatchResponse,
    pub(super) exit: DispatchExit,
}

impl DispatchBoundary {
    /// Emits the statement one stage uses to leave the dispatch with `value`.
    ///
    /// The labeled form binds the response first so the `break` value can never
    /// be mistaken for a labeled expression, whatever the response tokens are.
    pub(super) fn exit(self, value: &TokenStream2) -> TokenStream2 {
        match self.exit {
            DispatchExit::Return => quote! { return #value; },
            DispatchExit::Break => {
                let label = dispatch_label();
                quote! {
                    {
                        let __routerama_exit = #value;
                        break #label __routerama_exit;
                    }
                }
            }
        }
    }
}

pub(super) struct StaticRouteEntry {
    pub(super) handler: usize,
    pub(super) attribute: Attribute,
    pub(super) route: Route,
    pub(super) capture_keys: Vec<String>,
    pub(super) priority: Option<RoutePriority>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ExtractionKind {
    Parts,
    Body,
}

pub(super) struct CatchBinding {
    pub(super) kind: ExtractionKind,
    pub(super) extractor_key: String,
    pub(super) catcher: usize,
}

#[derive(Default)]
pub(super) struct RoutingPolicy {
    pub(super) fallback: Option<FallbackPolicy>,
    pub(super) catchers: Vec<CatcherPolicy>,
    pub(super) bindings: Vec<CatchBinding>,
    pub(super) befores: Vec<BeforeInterceptor>,
    pub(super) afters: Vec<AfterInterceptor>,
    pub(super) transforms: Vec<TransformInterceptor>,
}

impl RoutingPolicy {
    /// Router-wide request interceptors run at every entry, before routing.
    pub(super) fn router_wide_befores(&self) -> impl Iterator<Item = &BeforeInterceptor> {
        self.befores.iter().filter(|before| before.handlers.is_none())
    }

    /// Per-handler request interceptors run after route selection.
    pub(super) fn handler_befores(&self, handler: &Ident) -> impl Iterator<Item = &BeforeInterceptor> {
        self.befores.iter().filter(move |before| {
            before
                .handlers
                .as_ref()
                .is_some_and(|names| names.iter().any(|name| name == handler))
        })
    }

    /// Response interceptors that observe every generated response. They run
    /// outermost, at the entry, after any per-handler ones.
    pub(super) fn generated_wide_afters(&self) -> impl Iterator<Item = &AfterInterceptor> {
        self.afters.iter().filter(|after| after.handlers.is_none())
    }

    /// Per-handler response interceptors run innermost.
    pub(super) fn handler_afters(&self, handler: &Ident) -> impl Iterator<Item = &AfterInterceptor> {
        self.afters.iter().filter(move |after| {
            after
                .handlers
                .as_ref()
                .is_some_and(|names| names.iter().any(|name| name == handler))
        })
    }

    /// The single terminal body transform selected for a handler, if any.
    pub(super) fn transform_for(&self, handler: &Ident) -> Option<&TransformInterceptor> {
        self.transforms
            .iter()
            .find(|transform| transform.handlers.iter().any(|name| name == handler))
    }

    /// The concrete request body a handler's `#[body]` parameter extracts from.
    ///
    /// This is a transform replacement when one exists, otherwise the transport
    /// body.
    pub(super) fn body_input_for(&self, handler: &Ident, body_type: &Ident) -> TokenStream2 {
        self.transform_for(handler)
            .and_then(|transform| transform.replacement_body.as_ref())
            .map_or_else(|| quote! { #body_type }, |replacement| quote! { #replacement })
    }

    /// True when a `#[before]` interceptor may mutate the request head.
    ///
    /// The entry must then bind it mutably. Transforms read the head immutably.
    pub(super) fn mutates_request(&self) -> bool {
        !self.befores.is_empty()
    }
}

pub(super) struct FallbackPolicy {
    pub(super) method: Ident,
    pub(super) response_type: Type,
}

pub(super) struct CatcherPolicy {
    pub(super) method: Ident,
    pub(super) rejection_type: Type,
    pub(super) extractor_type: Option<Type>,
    pub(super) response_type: Type,
    pub(super) span: Span,
}

/// A generated `#[before]` request interceptor.
pub(super) struct BeforeInterceptor {
    pub(super) method: Ident,
    pub(super) response_type: Type,
    /// `None` is router-wide; `Some` lists the handlers it guards.
    pub(super) handlers: Option<Vec<Ident>>,
}

/// A generated `#[after]` response interceptor.
pub(super) struct AfterInterceptor {
    pub(super) method: Ident,
    /// `None` observes every generated response; `Some` lists the handlers it post-processes.
    pub(super) handlers: Option<Vec<Ident>>,
}

/// A generated `#[transform]` terminal request-body interceptor.
pub(super) struct TransformInterceptor {
    pub(super) method: Ident,
    pub(super) mode: TransformMode,
    pub(super) handlers: Vec<Ident>,
    /// `Some` produces a replacement body; `None` consumes the body.
    ///
    /// A streaming transform's replacement already names the generated
    /// transport body type, because the interceptor's generic body parameter is
    /// substituted when the signature is validated.
    pub(super) replacement_body: Option<Type>,
    /// Streaming method predicates, with the body parameter already rewritten.
    ///
    /// The parameter is replaced by the generated transport body type.
    pub(super) body_bounds: Vec<WherePredicate>,
    pub(super) response_type: Type,
    pub(super) span: Span,
}

/// How a `#[transform]` interceptor takes ownership of the request body.
pub(super) enum TransformMode {
    /// `#[transform(limit = N, ...)]` collects the body into bounded `bytes::Bytes`.
    ///
    /// The collection happens before the interceptor is called.
    Buffered { limit: syn::Expr },
    /// `#[transform(stream, ...)]` moves the transport body into an interceptor.
    ///
    /// The interceptor is generic over that body, so nothing is buffered by the
    /// framework.
    Streaming,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HandlerKind {
    Static,
    Dynamic,
}

pub(super) enum Argument {
    Capture(Ident),
    Parts(Ident, Type),
    Body(Ident, Type),
}

pub(super) struct GeneratedIdents {
    pub(super) request: Ident,
    pub(super) state: Ident,
    pub(super) parts: Ident,
    pub(super) body: Ident,
    pub(super) route: Ident,
    pub(super) response: Ident,
    pub(super) failure: Ident,
}

pub(super) struct RouteContract {
    pub(super) bounds: Vec<TokenStream2>,
}

pub(super) struct SharedState {
    pub(super) ty: Type,
    pub(super) generic: Option<Ident>,
}

pub(super) struct ResponseBodyModel {
    pub(super) body: Ident,
    pub(super) projection: Ident,
    pub(super) error: Ident,
    pub(super) sources: Vec<ResponseSource>,
    pub(super) heterogeneous_data: bool,
}

pub(super) struct ResponseSource {
    pub(super) key: String,
    pub(super) variant: Ident,
    pub(super) body_type: Ident,
    pub(super) error_type: Ident,
    pub(super) label: String,
}
