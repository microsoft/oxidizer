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
//! the operating system already publishes.
//!
//! Locating these constants here also keeps their import path honest: an
//! import of `crate::bindings::WINHTTP_OPTION_CONTEXT_VALUE` tells the reader
//! the value comes straight from the SDK, whereas the same name imported from
//! a policy module would suggest the importing layer derives it. The one
//! constant this module defines rather than re-exports carries that distinction
//! in its own documentation.

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

/// Session option bounding how long an idle pooled connection stays eligible
/// for reuse.
///
/// Not published in the public `winhttp.h` surface the `windows` crate binds,
/// so this crate defines the constant itself. Semantics below come from the
/// Windows connection-manager implementation
/// (`onecore/net/winhttp/inc/hinet.hxx`, `defaults.h`, and
/// `onecore/net/webio/core/connmgr.c`); a near-duplicate manager elsewhere in
/// the tree uses the same names with a different expiry predicate, so those
/// paths are intentional.
///
/// The option is a `DWORD` millisecond count on a session handle. The session
/// must not yet have a child handle, and the value must be at least five
/// seconds; either violation is an error rather than a clamp. That is why
/// [`crate::session`] sets it during construction and
/// [`crate::convert::connection_idle_timeout_millis`] raises shorter windows.
/// There is no upper bound: the largest `DWORD` is an ordinary multi-week
/// window. Setting the option also disables the process-wide keep-alive pool,
/// which this transport disables explicitly anyway.
///
/// One connection-manager field backs HTTP/1.1, HTTP/2, and HTTP/3 reuse.
/// Windows defaults the unset option to one minute, matching `fetch`'s
/// `ConnectionIdleTimeout` default. Connections past the window stop being
/// reused on the next lookup; socket release follows that lookup or a periodic
/// sweep and is not prompt, so the option bounds reuse rather than teardown
/// latency.
pub(crate) const WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT: u32 = 135;
