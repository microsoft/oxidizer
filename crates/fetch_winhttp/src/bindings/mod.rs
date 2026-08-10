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
/// This is the sole constant the crate defines instead of re-exporting, because
/// the operating system does not publish it: the Windows header declares it
/// inside the `;begin_internal` block of `onecore/net/published/inc/winhttp.w`,
/// so it never reaches the public `winhttp.h` surface the `windows` bindings are
/// generated from, and there is no SDK item to re-export and no reference page
/// to link. Everything below was therefore read from the Windows source, whose
/// relevant parts are `onecore/net/winhttp/inc/hinet.hxx` (the setter and its
/// preconditions), `onecore/net/winhttp/inc/defaults.h` (the bounds), and
/// `onecore/net/webio/core/connmgr.c` (the sweep and the lookup path). The
/// `onecore/` prefixes matter: a near-duplicate connection manager exists
/// elsewhere in the tree with its own copy of these names and a subtly
/// different expiry predicate. This block stands in for the documentation the
/// option does not have, so it states the contract in full.
///
/// The option takes a `DWORD` count of milliseconds and applies to a session
/// handle. Its preconditions are that the session has not yet created a child
/// handle and that the value is at least five seconds; violating either is an
/// error rather than a clamp, which is why [`crate::session`] sets it during
/// session construction and [`crate::convert::connection_idle_timeout_millis`]
/// raises shorter windows. There is no upper bound: the setter range-checks
/// only the minimum, and the sweep compares a 64-bit elapsed-time delta against
/// the configured value, so the largest `DWORD` is an ordinary window of about
/// fifty days rather than a value that overflows or inverts. Setting the option
/// also disables the process-wide keep-alive pool, which this transport
/// disables explicitly in any case, so the two settings agree.
///
/// A single connection-manager field backs the sweep for HTTP/1.1, HTTP/2, and
/// HTTP/3 alike, so this one value governs reuse eligibility across every
/// protocol the transport negotiates. Windows applies its own default when the
/// option is unset, and that default is one minute - the same duration
/// `fetch`'s own `ConnectionIdleTimeout` defaults to, so a caller who
/// configures nothing sees identical behavior whether or not this option is
/// set.
///
/// Connections past the window stop being reused immediately, because the
/// lookup path re-checks the window when it selects a connection. Their sockets
/// are released either by that same lookup, which tears down the expired
/// connections it walks past, or by a periodic sweep for connections no lookup
/// reaches, whichever comes first. Neither schedule is prompt, so this option
/// bounds reuse rather than promising timely socket release.
pub(crate) const WINHTTP_OPTION_CONNECTION_IDLE_TIMEOUT: u32 = 135;
