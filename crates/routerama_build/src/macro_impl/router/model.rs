// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The parsed shape of a `#[router]` annotated inherent impl.

use alloc::string::String;
use alloc::vec::Vec;

use proc_macro2::{Ident, Span};
use syn::{LitStr, Type};

/// The `#[router(...)]` attribute arguments.
pub(crate) struct RouterArgs {
    /// The fixed shared-state type, when `state = Type` was supplied.
    pub(crate) state: Option<Type>,
    /// The span of the `erased_mounts` marker, when supplied.
    pub(crate) erased_mounts: Option<Span>,
}

/// One `#[route(METHOD, "path", ...)]` declaration on a handler.
pub(crate) struct RouteDecl {
    /// The span of the attribute's `route` path, used by policy diagnostics.
    pub(crate) attr_span: Span,
    pub(crate) method: String,
    pub(crate) path: LitStr,
    pub(crate) host: Option<LitStr>,
    pub(crate) consumes: Option<LitStr>,
    pub(crate) produces: Option<LitStr>,
    pub(crate) priority: Option<(i32, Span)>,
}

impl RouteDecl {
    pub(crate) const fn has_predicates(&self) -> bool {
        self.host.is_some() || self.consumes.is_some() || self.produces.is_some()
    }

    pub(crate) fn predicates(&self) -> (Option<String>, Option<String>, Option<String>) {
        (
            self.host.as_ref().map(LitStr::value),
            self.consumes.as_ref().map(LitStr::value),
            self.produces.as_ref().map(LitStr::value),
        )
    }
}

/// How a handler parameter is supplied.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamKind {
    /// A static path capture named by the route template.
    Capture,
    /// A `#[capture]`-marked parameter of a configured dynamic route.
    DynamicCapture,
    /// The single `#[body]`-marked parameter.
    Body,
    /// An ordinary request-parts extractor.
    Parts,
}

pub(crate) struct Param {
    pub(crate) name: Ident,
    pub(crate) ty: Type,
    pub(crate) kind: ParamKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandlerKind {
    Static,
    Dynamic,
}

pub(crate) struct Handler {
    /// The impl method name.
    pub(crate) name: Ident,
    pub(crate) kind: HandlerKind,
    pub(crate) params: Vec<Param>,
    pub(crate) response: Type,
    pub(crate) routes: Vec<RouteDecl>,
}

impl Handler {
    pub(crate) fn has_body(&self) -> bool {
        self.params.iter().any(|param| param.kind == ParamKind::Body)
    }
}

/// A `#[before]`, `#[after]`, or `#[transform]` method.
pub(crate) struct Interceptor {
    pub(crate) name: Ident,
    pub(crate) attr_span: Span,
    pub(crate) kind: InterceptorKind,
    /// The handler names this interceptor applies to; empty means router-wide.
    pub(crate) handlers: Vec<Ident>,
    /// The replacement request-body type a transform hands to `#[body]`
    /// extraction, with a streaming transform's generic parameter already
    /// rewritten to the generated transport body parameter.
    pub(crate) replacement: Option<Type>,
    /// The short-circuit response type this interceptor can produce.
    pub(crate) short_circuit: Option<Type>,
    /// The bounds a streaming transform declares on the transport request
    /// body, which the generated entry point must forward.
    pub(crate) transport_bounds: Vec<proc_macro2::TokenStream>,
}

pub(crate) enum InterceptorKind {
    Before,
    After,
    /// A buffered transform with its explicit byte limit.
    TransformBuffered {
        limit: syn::Expr,
        consumes: bool,
    },
    /// A streaming transform generic over the transport request body.
    TransformStream {
        consumes: bool,
    },
}

/// A `#[catch(Rejection)]` or `#[catch(Rejection, from = Extractor)]` method.
pub(crate) struct Catcher {
    pub(crate) name: Ident,
    pub(crate) attr_span: Span,
    /// The base identifier of the caught rejection type.
    pub(crate) rejection_base: String,
    /// The base identifier of the `from = Extractor` type, when supplied.
    pub(crate) from_base: Option<String>,
    /// The catcher's by-value rejection parameter type.
    pub(crate) parameter: Type,
    /// The catcher's declared response type.
    pub(crate) response: Type,
}

/// The complete parsed router.
pub(crate) struct Router {
    pub(crate) args: RouterArgs,
    pub(crate) service_ty: Type,
    pub(crate) service_name: Ident,
    pub(crate) handlers: Vec<Handler>,
    pub(crate) interceptors: Vec<Interceptor>,
    pub(crate) fallback: Option<(Ident, Type)>,
    pub(crate) catchers: Vec<Catcher>,
}
