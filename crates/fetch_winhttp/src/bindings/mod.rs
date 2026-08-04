// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owns this crate's boundary with the `WinHTTP` operating-system API
//! (implementation.md section 1).
//!
//! Two kinds of thing cross that boundary and both are defined here. Entry
//! points are abstracted behind the [`Bindings`] trait and dispatched through
//! [`BindingsFacade`], which is what allows every request lifecycle to be
//! driven against mock bindings with no network and no real handles
//! (implementation.md section 7.1). SDK constants are re-exported below from
//! the generated `windows` bindings, so no module duplicates a numeric value
//! defined by the operating system.
//!
//! Locating these constants here also keeps their import path honest: an
//! import of `crate::bindings::WINHTTP_OPTION_CONTEXT_VALUE` tells the reader
//! the value comes straight from the SDK, whereas the same name imported from
//! a policy module would suggest the importing layer derives it.

mod abstractions;
mod facade;
mod real;

#[cfg(test)]
pub(crate) use abstractions::MockBindings;
pub(crate) use abstractions::{Bindings, StatusCallback};
pub(crate) use facade::BindingsFacade;
// `WinHTTP` flag, option, and query constants used across the crate. This is a
// re-export, not a redefinition: the values come from the generated `windows`
// bindings, and routing them through this module gives every consumer a single
// import path that names the FFI boundary they cross. A module is still free to
// import a constant directly from `windows` when the constant is local to that
// module's own concern and adding it here would only lengthen this list.
pub(crate) use windows::Win32::Networking::WinHttp::{
    WINHTTP_FLAG_ASYNC, WINHTTP_FLAG_AUTOMATIC_CHUNKING, WINHTTP_FLAG_SECURE, WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH,
    WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_DECOMPRESSION, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
    WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED, WINHTTP_OPTION_HTTP_PROTOCOL_USED, WINHTTP_OPTION_HTTP2_KEEPALIVE,
    WINHTTP_OPTION_HTTP3_KEEPALIVE, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_OPTION_SECURITY_FLAGS,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_FLAG_WIRE_ENCODING, WINHTTP_QUERY_RAW_HEADERS_CRLF, WINHTTP_QUERY_STATUS_CODE,
    WINHTTP_QUERY_VERSION,
};
