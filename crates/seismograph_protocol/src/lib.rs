// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Local monitor protocol and discovery model for Seismograph.

mod codec;
pub mod message;
pub mod monitor;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::{fmt, io};

use message::{Request, Response};

const FRAME_MAGIC: [u8; 4] = *b"SGMP";
const FRAME_HEADER_BYTES: usize = 20;
const MAX_CONTROL_BYTES: usize = 64 * 1024;
const MAX_SNAPSHOT_BYTES: usize = u32::MAX as usize;

const VERSION: u16 = 7;

/// Protocol or discovery failure.
#[derive(Debug)]
pub enum Error {
    /// Underlying I/O failure.
    Io(io::Error),
    /// Frame or request payload is malformed.
    InvalidMessage,
    /// Discovery descriptor is malformed.
    InvalidDescriptor,
    /// Peer or descriptor uses an unsupported protocol version.
    UnsupportedVersion,
    /// Payload exceeds the protocol limit.
    MessageTooLarge,
    /// No secure per-user runtime directory is available.
    MissingRuntimeDirectory,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::InvalidMessage => f.write_str("invalid Seismograph monitor message"),
            Self::InvalidDescriptor => f.write_str("invalid Seismograph monitor descriptor"),
            Self::UnsupportedVersion => f.write_str("unsupported Seismograph monitor protocol version"),
            Self::MessageTooLarge => f.write_str("Seismograph monitor message exceeds its size limit"),
            Self::MissingRuntimeDirectory => f.write_str("no per-user runtime directory is available"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Writes one request frame.
///
/// # Errors
///
/// Returns an error when the request is too large or writing fails.
pub fn write_request(writer: &mut impl Write, request_id: u64, request: &Request) -> Result<(), Error> {
    let (kind, payload) = message::encode_request(request);
    write_frame(writer, kind, request_id, &payload)
}

/// Reads one request frame.
///
/// # Errors
///
/// Returns an error when reading fails or the request is malformed.
pub fn read_request(reader: &mut impl Read) -> Result<(u64, Request), Error> {
    let frame = read_frame(reader, MAX_CONTROL_BYTES)?;
    message::decode_request(frame.kind, &frame.payload).map(|request| (frame.request_id, request))
}

/// Writes one response frame.
///
/// # Errors
///
/// Returns an error when the response is too large or writing fails.
pub fn write_response(writer: &mut impl Write, request_id: u64, response: &Response) -> Result<(), Error> {
    let (kind, payload) = message::encode_response(response)?;
    write_frame(writer, kind, request_id, &payload)
}

/// Reads one response frame.
///
/// # Errors
///
/// Returns an error when reading fails or the response is malformed.
pub fn read_response(reader: &mut impl Read) -> Result<(u64, Response), Error> {
    let frame = read_frame(reader, MAX_SNAPSHOT_BYTES)?;
    message::decode_response(frame.kind, &frame.payload).map(|response| (frame.request_id, response))
}

/// Returns the per-user monitor discovery directory.
///
/// # Errors
///
/// Returns an error when the platform does not expose a per-user directory.
pub fn monitor_directory() -> Result<PathBuf, Error> {
    platform_monitor_directory()
}

fn write_frame(writer: &mut impl Write, kind: u16, request_id: u64, payload: &[u8]) -> Result<(), Error> {
    let payload_len = u32::try_from(payload.len()).map_err(|_error| Error::MessageTooLarge)?;
    writer.write_all(&FRAME_MAGIC).map_err(Error::Io)?;
    writer.write_all(&VERSION.to_le_bytes()).map_err(Error::Io)?;
    writer.write_all(&kind.to_le_bytes()).map_err(Error::Io)?;
    writer.write_all(&request_id.to_le_bytes()).map_err(Error::Io)?;
    writer.write_all(&payload_len.to_le_bytes()).map_err(Error::Io)?;
    writer.write_all(payload).map_err(Error::Io)?;
    writer.flush().map_err(Error::Io)
}

fn read_frame(reader: &mut impl Read, maximum: usize) -> Result<Frame, Error> {
    let mut header = [0; FRAME_HEADER_BYTES];
    reader.read_exact(&mut header).map_err(Error::Io)?;
    if header[..4] != FRAME_MAGIC {
        return Err(Error::InvalidMessage);
    }
    if u16::from_le_bytes([header[4], header[5]]) != VERSION {
        return Err(Error::UnsupportedVersion);
    }
    let kind = u16::from_le_bytes([header[6], header[7]]);
    let mut request_id = [0; 8];
    request_id.copy_from_slice(&header[8..16]);
    let request_id = u64::from_le_bytes(request_id);
    let mut len = [0; 4];
    len.copy_from_slice(&header[16..20]);
    let len = u32::from_le_bytes(len) as usize;
    if len > maximum {
        return Err(Error::MessageTooLarge);
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload).map_err(Error::Io)?;
    Ok(Frame { kind, request_id, payload })
}

#[cfg(target_os = "windows")]
fn platform_monitor_directory() -> Result<PathBuf, Error> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or(Error::MissingRuntimeDirectory)?;
    Ok(PathBuf::from(local).join("seismograph").join("monitor"))
}

#[cfg(unix)]
fn platform_monitor_directory() -> Result<PathBuf, Error> {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(runtime).join("seismograph"));
    }
    // SAFETY: geteuid has no preconditions and does not access Rust-owned memory.
    let user_id = unsafe { libc::geteuid() };
    Ok(std::env::temp_dir().join(format!("seismograph-{user_id}")))
}

#[cfg(not(any(target_os = "windows", unix)))]
fn platform_monitor_directory() -> Result<PathBuf, Error> {
    Ok(std::env::temp_dir().join("seismograph"))
}

struct Frame {
    kind: u16,
    request_id: u64,
    payload: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_expose_descriptive_messages_and_io_sources() {
        let io = Error::Io(io::Error::other("transport failed"));
        assert_eq!(io.to_string(), "transport failed");
        assert!(std::error::Error::source(&io).is_some());

        let cases = [
            (Error::InvalidMessage, "invalid Seismograph monitor message"),
            (Error::InvalidDescriptor, "invalid Seismograph monitor descriptor"),
            (Error::UnsupportedVersion, "unsupported Seismograph monitor protocol version"),
            (Error::MessageTooLarge, "Seismograph monitor message exceeds its size limit"),
            (Error::MissingRuntimeDirectory, "no per-user runtime directory is available"),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    #[test]
    fn malformed_frame_headers_are_rejected() {
        let mut invalid_magic = [0_u8; FRAME_HEADER_BYTES];
        invalid_magic[4..6].copy_from_slice(&VERSION.to_le_bytes());
        assert!(matches!(
            read_frame(&mut invalid_magic.as_slice(), MAX_CONTROL_BYTES),
            Err(Error::InvalidMessage)
        ));

        let mut unsupported_version = [0_u8; FRAME_HEADER_BYTES];
        unsupported_version[..4].copy_from_slice(&FRAME_MAGIC);
        unsupported_version[4..6].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(matches!(
            read_frame(&mut unsupported_version.as_slice(), MAX_CONTROL_BYTES),
            Err(Error::UnsupportedVersion)
        ));

        let mut oversized = [0_u8; FRAME_HEADER_BYTES];
        oversized[..4].copy_from_slice(&FRAME_MAGIC);
        oversized[4..6].copy_from_slice(&VERSION.to_le_bytes());
        oversized[16..20].copy_from_slice(&2_u32.to_le_bytes());
        assert!(matches!(read_frame(&mut oversized.as_slice(), 1), Err(Error::MessageTooLarge)));
    }

    #[test]
    fn monitor_directory_is_platform_specific() {
        let directory = monitor_directory().unwrap();
        assert_eq!(directory.file_name().unwrap(), "monitor");
    }
}
