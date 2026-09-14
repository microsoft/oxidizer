// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! WebSocket handshake keys, versions, protocols, and extensions.

mod sec_web_socket_accept;
mod sec_web_socket_extensions;
mod sec_web_socket_key;
mod sec_web_socket_protocol;
mod sec_web_socket_version;
mod shared;

#[doc(inline)]
pub use sec_web_socket_accept::{SecWebSocketAccept, SecWebSocketAcceptOwned, SecWebSocketAcceptView};
#[doc(inline)]
pub use sec_web_socket_extensions::{
    SecWebSocketExtensions, SecWebSocketExtensionsBuilder, SecWebSocketExtensionsOwned, SecWebSocketExtensionsView,
    WebSocketExtensionParameterView, WebSocketExtensionParameters, WebSocketExtensionView,
};
#[doc(inline)]
pub use sec_web_socket_key::{SecWebSocketKey, SecWebSocketKeyOwned, SecWebSocketKeyView};
#[doc(inline)]
pub use sec_web_socket_protocol::{SecWebSocketProtocol, SecWebSocketProtocolOwned, SecWebSocketProtocolView};
#[doc(inline)]
pub use sec_web_socket_version::{SecWebSocketVersion, SecWebSocketVersionOwned};
