// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thiserror::Error;
#[cfg(target_family = "windows")]
use windows::Win32::Networking::WinSock::WSA_ERROR;

/// A specialized `Result` type for Oxidizer Runtime operations
/// that return an Oxidizer Runtime [`Error`][enum@Error] on failure.
pub type Result<T> = std::result::Result<T, Error>;

/// An error originating in the Oxidizer Runtime.
///
/// This is an umbrella type for all kinds of errors that can be returned by the Oxidizer Runtime,
/// including programming errors (e.g. invalid arguments) and errors from the environment (e.g. file
/// not found, connection lost).
///
/// Specific enum variants may provide additional detail and expose underlying operating system
/// status codes to help react to specific conditions. Future versions may add additional enum
/// variants.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The caller of some API made a mistake (e.g. supplied invalid arguments or called an
    /// operation out of sequence).
    #[error("{0}")]
    Programming(String),

    /// We are re-packaging an error from a Windows Sockets API call
    /// without adding further details in the Oxidizer Runtime layer.
    ///
    /// You may find useful status codes in the fields of the error object.
    #[error("WinSock error {} ({})", .code, .detail.0)]
    #[cfg(target_family = "windows")]
    Winsock { code: i32, detail: WSA_ERROR },

    /// We are re-packaging an error from a Windows API call
    /// without adding further details in the Oxidizer Runtime layer.
    ///
    /// You may find useful status codes in the inner error object.
    #[error(transparent)]
    #[cfg(target_family = "windows")]
    Windows(#[from] windows::core::Error),

    /// We are re-packaging an error from the Rust standard library I/O logic
    /// without adding further details in the Oxidizer Runtime layer.
    #[error(transparent)]
    StdIo(#[from] std::io::Error),

    /// We are re-packaging an error we obtained from some downstream mechanism
    /// without adding further details in the Oxidizer Runtime layer.
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}