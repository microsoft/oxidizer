// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Local monitor protocol, OS-authorized transport, and discovery for Seismograph.
//!
//! Monitors use pathname Unix sockets or local Windows named pipes. Connections
//! are authorized by the operating system for the owning user and root/elevated
//! administrators, without authentication secrets. A token-free Hello identifies
//! the process instance and returns its recording configuration.
//!
//! [`discovery::Directory::current`] automatically creates a private registry
//! for the current user, without administrator setup. Unix prefers
//! `$XDG_RUNTIME_DIR/seismograph`, with a private randomized `/tmp` directory
//! as the fallback. Windows uses local application data. Discovery aggregates
//! accessible user registries; unsafe existing permissions are never relaxed.
//! Monitor protocol version 9 carries all six recording policies in each
//! configuration. Other protocol versions are rejected.
//! Saved telemetry snapshots use a separate format.
//!
//! Construct an [`io::MonitorIo`] at the application boundary and pass it to
//! discovery, descriptor, and transport constructors. Their returned objects
//! retain that capability.
//!
//! # Features
//!
//! - **`test-util`**: Provides supported, documented mock I/O through
//!   `io::MonitorIo::new_mocked` and `io::MonitorIoControl`, including isolated
//!   files and streams, injected failures, partial transfers, and controlled clocks.
//!   Enable this on a dev-dependency for application tests. Mocked I/O creates no
//!   host files or endpoints and does not prove native OS permission enforcement.
//!   The same feature enables `testing` for isolated native IPC tests, which
//!   create real temporary files and OS endpoints without using the installed
//!   user registries.
//!
//! Use [`read_control_response`] for automatic metadata probes: its 64 KiB
//! payload limit is checked before reading or allocating the advertised body.
//! Explicit snapshot capture can use [`read_response`] for larger payloads.

mod codec;
pub mod discovery;
pub mod io;
pub mod message;
pub mod monitor;
mod platform;
pub mod transport;

#[cfg(any(test, feature = "test-util"))]
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
pub mod testing;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::{fmt, io as std_io};

use message::{Request, Response};

const FRAME_MAGIC: [u8; 4] = *b"SGMP";
const FRAME_HEADER_BYTES: usize = 20;
const FRAME_READ_CHUNK_BYTES: usize = 8_192;
const MAX_CONTROL_BYTES: usize = 65_536;
const MAX_SNAPSHOT_BYTES: usize = u32::MAX as usize;

const VERSION: u16 = 9;

/// Protocol or discovery failure.
#[derive(Debug)]
pub enum Error {
    /// Underlying I/O failure.
    Io(std_io::Error),
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
    /// A registry operation failed.
    Registry {
        /// Registry path involved in the operation.
        path: PathBuf,
        /// Underlying filesystem or access-control failure.
        source: std_io::Error,
    },
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
            Self::Registry { path, source } => write!(f, "Seismograph monitor registry {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) | Self::Registry { source: error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Writes one request frame.
///
/// # Errors
///
/// Returns an error when the request is too large or writing fails.
/// After an I/O error, the writer may contain a partial frame and must be
/// discarded rather than reused for another frame.
pub fn write_request(writer: &mut impl Write, request_id: u64, request: &Request) -> Result<(), Error> {
    let (kind, payload) = message::encode_request(request);
    write_frame(writer, kind, request_id, &payload)
}

/// Reads one request frame.
///
/// # Errors
///
/// Returns an error when reading fails or the request is malformed.
/// After any error, frame alignment is not guaranteed and the reader must be
/// discarded rather than reused for another frame.
pub fn read_request(reader: &mut impl Read) -> Result<(u64, Request), Error> {
    let frame = read_frame(reader, MAX_CONTROL_BYTES)?;
    message::decode_request(frame.kind, &frame.payload).map(|request| (frame.request_id, request))
}

/// Writes one response frame.
///
/// # Errors
///
/// Returns an error when the response is too large or writing fails.
/// After an I/O error, the writer may contain a partial frame and must be
/// discarded rather than reused for another frame.
pub fn write_response(writer: &mut impl Write, request_id: u64, response: &Response) -> Result<(), Error> {
    let (kind, payload) = message::encode_response(response)?;
    write_frame(writer, kind, request_id, payload.as_ref())
}

/// Reads one response frame.
///
/// Permits large snapshot payloads. Use [`read_control_response`] for bounded
/// automatic metadata probes.
///
/// # Errors
///
/// Returns an error when reading fails or the response is malformed.
/// After any error, frame alignment is not guaranteed and the reader must be
/// discarded rather than reused for another frame.
pub fn read_response(reader: &mut impl Read) -> Result<(u64, Response), Error> {
    #[cfg(target_pointer_width = "64")]
    let maximum = { MAX_SNAPSHOT_BYTES };
    #[cfg(not(target_pointer_width = "64"))]
    let maximum = MAX_SNAPSHOT_BYTES;
    let frame = read_frame(reader, maximum)?;
    message::decode_response(frame.kind, &frame.payload).map(|response| (frame.request_id, response))
}

/// Reads one response with a 64 KiB control-payload limit.
///
/// Checks the advertised length before reading or allocating the body, even
/// when the peer advertises an unexpected snapshot response. Use
/// [`read_response`] for explicit snapshot capture.
///
/// # Errors
///
/// Returns [`Error::MessageTooLarge`] for a larger payload, or an error when
/// reading or decoding fails. Discard the reader after an error.
pub fn read_control_response(reader: &mut impl Read) -> Result<(u64, Response), Error> {
    let frame = read_frame(reader, MAX_CONTROL_BYTES)?;
    message::decode_response(frame.kind, &frame.payload).map(|response| (frame.request_id, response))
}

fn message_length(length: usize) -> Result<u32, Error> {
    u32::try_from(length).map_err(|_error| Error::MessageTooLarge)
}

fn write_frame(writer: &mut impl Write, kind: u16, request_id: u64, payload: &[u8]) -> Result<(), Error> {
    write_frame_with_length(writer, kind, request_id, payload.len(), |writer| writer.write_all(payload))
}

fn write_frame_with_length<W: Write>(
    writer: &mut W,
    kind: u16,
    request_id: u64,
    payload_length: usize,
    write_payload: impl FnOnce(&mut W) -> std_io::Result<()>,
) -> Result<(), Error> {
    let payload_len = message_length(payload_length)?;
    writer.write_all(&FRAME_MAGIC).map_err(Error::Io)?;
    writer.write_all(&VERSION.to_le_bytes()).map_err(Error::Io)?;
    writer.write_all(&kind.to_le_bytes()).map_err(Error::Io)?;
    writer.write_all(&request_id.to_le_bytes()).map_err(Error::Io)?;
    writer.write_all(&payload_len.to_le_bytes()).map_err(Error::Io)?;
    write_payload(writer).map_err(Error::Io)?;
    writer.flush().map_err(Error::Io)
}

fn read_frame(reader: &mut impl Read, maximum: usize) -> Result<Frame, Error> {
    read_frame_with_reserve(reader, maximum, Vec::try_reserve)
}

fn read_frame_with_reserve(
    reader: &mut impl Read,
    maximum: usize,
    mut reserve: impl FnMut(&mut Vec<u8>, usize) -> Result<(), std::collections::TryReserveError>,
) -> Result<Frame, Error> {
    // #[gamma::skip(literal.int_increment, reason = "read_exact initializes the entire header or returns before any header field is read")]
    let mut header = [0; FRAME_HEADER_BYTES];
    reader.read_exact(&mut header).map_err(Error::Io)?;
    if header[..4] != FRAME_MAGIC {
        return Err(Error::InvalidMessage);
    }
    if u16::from_le_bytes([header[4], header[5]]) != VERSION {
        return Err(Error::UnsupportedVersion);
    }
    let kind = u16::from_le_bytes([header[6], header[7]]);
    // #[gamma::skip(literal.int_increment, reason = "copy_from_slice overwrites the entire request ID before it is read")]
    let mut request_id = [0; 8];
    request_id.copy_from_slice(&header[8..16]);
    let request_id = u64::from_le_bytes(request_id);
    // #[gamma::skip(literal.int_increment, reason = "copy_from_slice overwrites the entire length before it is read")]
    let mut len = [0; 4];
    len.copy_from_slice(&header[16..20]);
    let len = u32::from_le_bytes(len) as usize;
    if len > maximum {
        return Err(Error::MessageTooLarge);
    }
    let mut payload = Vec::new();
    // #[gamma::skip(literal.int_increment, reason = "each read_exact overwrites every byte copied into payload; failed reads return before copying")]
    let mut chunk = [0_u8; FRAME_READ_CHUNK_BYTES];
    for _ in 0..len / chunk.len() {
        reader.read_exact(&mut chunk).map_err(Error::Io)?;
        reserve(&mut payload, chunk.len()).map_err(|_error| Error::MessageTooLarge)?;
        payload.extend_from_slice(&chunk);
    }
    let tail_len = len % chunk.len();
    // #[gamma::skip(cond.always_true, reason = "a zero-length tail performs only an empty read_exact, reserve of zero, and empty extension; it consumes no bytes and preserves the payload")]
    if tail_len != 0 {
        reader.read_exact(&mut chunk[..tail_len]).map_err(Error::Io)?;
        reserve(&mut payload, tail_len).map_err(|_error| Error::MessageTooLarge)?;
        payload.extend_from_slice(&chunk[..tail_len]);
    }
    Ok(Frame { kind, request_id, payload })
}

struct Frame {
    kind: u16,
    request_id: u64,
    payload: Vec<u8>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn november_public_recording_frames_preserve_each_distinct_sampling_byte_and_request_identity() {
        use crate::message::{RecordingConfiguration, RecordingPolicy};

        for sampling in [0x0001_0203, 0x0003_0405, 0x0007_0605] {
            let request = Request::SetRecording(RecordingConfiguration {
                allocations: RecordingPolicy {
                    enabled: true,
                    capture_backtraces: false,
                    sampling_one_in: sampling,
                },
                ..RecordingConfiguration::default()
            });
            let mut bytes = Vec::new();
            write_request(&mut bytes, 0x0102_0304_0506_0708, &request).unwrap();
            assert_eq!(bytes[FRAME_HEADER_BYTES + 6..FRAME_HEADER_BYTES + 10], sampling.to_le_bytes());
            assert_eq!(read_request(&mut bytes.as_slice()).unwrap(), (0x0102_0304_0506_0708, request));
        }
    }

    #[test]
    fn november_frame_allocation_failures_propagate_before_copying_each_chunk_and_tail_without_large_allocations() {
        let payload = vec![17; FRAME_READ_CHUNK_BYTES + 1];
        let mut bytes = Vec::new();
        write_frame(&mut bytes, 1, 23, &payload).unwrap();
        for fail_at in [0, 1] {
            let mut reader = io::Cursor::new(&bytes);
            let mut requests = Vec::new();
            let result = read_frame_with_reserve(&mut reader, MAX_SNAPSHOT_BYTES, |buffer, additional| {
                requests.push((buffer.len(), additional));
                if requests.len() - 1 == fail_at {
                    return Err(Vec::<u8>::new().try_reserve(usize::MAX).unwrap_err());
                }
                buffer.try_reserve(additional)
            });
            assert!(matches!(result, Err(Error::MessageTooLarge)));
            let expected = [(0, FRAME_READ_CHUNK_BYTES), (FRAME_READ_CHUNK_BYTES, 1)];
            assert_eq!(requests, expected[..=fail_at]);
            assert_eq!(
                reader.position(),
                u64::try_from(FRAME_HEADER_BYTES + FRAME_READ_CHUNK_BYTES + fail_at).unwrap()
            );
        }
    }

    #[test]
    fn november_frame_length_admission_rejects_unrepresentable_payload_before_any_header_or_payload_write() {
        let Some(length) = (u32::MAX as usize).checked_add(1) else {
            return;
        };
        let mut bytes = Vec::new();
        let error = write_frame_with_length(&mut bytes, 1, 23, length, |_| panic!("invalid length must not publish payload")).unwrap_err();
        assert!(matches!(error, Error::MessageTooLarge));
        assert!(bytes.is_empty());
    }

    #[test]
    fn control_responses_reject_large_advertisements_after_the_header() {
        let mut header = Vec::new();
        write_response(&mut header, 1, &Response::Snapshot(Vec::new())).unwrap();
        for length in [u32::try_from(MAX_CONTROL_BYTES + 1).unwrap(), u32::MAX] {
            header[16..20].copy_from_slice(&length.to_le_bytes());
            let mut reader = io::Cursor::new(&header);
            assert!(matches!(read_control_response(&mut reader), Err(Error::MessageTooLarge)));
            assert_eq!(reader.position(), 20);
        }
    }

    #[test]
    fn control_response_boundary_does_not_restrict_explicit_snapshots() {
        let response = Response::Snapshot(vec![7; MAX_CONTROL_BYTES]);
        let mut frame = Vec::new();
        write_response(&mut frame, 2, &response).unwrap();
        assert_eq!(read_control_response(&mut frame.as_slice()).unwrap(), (2, response));
        let response = Response::Snapshot(vec![9; MAX_CONTROL_BYTES + 1]);
        frame.clear();
        write_response(&mut frame, 2, &response).unwrap();
        assert_eq!(read_response(&mut frame.as_slice()).unwrap(), (2, response));
        assert!(matches!(read_control_response(&mut frame.as_slice()), Err(Error::MessageTooLarge)));
    }

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

        let mut truncated_snapshot = [0_u8; FRAME_HEADER_BYTES];
        truncated_snapshot[..4].copy_from_slice(&FRAME_MAGIC);
        truncated_snapshot[4..6].copy_from_slice(&VERSION.to_le_bytes());
        truncated_snapshot[6..8].copy_from_slice(&103_u16.to_le_bytes());
        truncated_snapshot[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read_response(&mut truncated_snapshot.as_slice()), Err(Error::Io(_))));
    }

    #[test]
    fn frame_size_boundaries_and_chunking_are_exact() {
        assert_eq!(
            (FRAME_HEADER_BYTES, FRAME_READ_CHUNK_BYTES, MAX_CONTROL_BYTES),
            (20, 8 * 1024, 64 * 1024)
        );

        for payload_len in [0, FRAME_READ_CHUNK_BYTES - 1, FRAME_READ_CHUNK_BYTES, FRAME_READ_CHUNK_BYTES + 1] {
            let payload = vec![0xA5; payload_len];
            let mut encoded = Vec::new();
            write_frame(&mut encoded, 103, 42, &payload).unwrap();
            assert_eq!(encoded.len(), FRAME_HEADER_BYTES + payload_len);
            let decoded = read_frame(&mut encoded.as_slice(), payload_len).unwrap();
            assert_eq!((decoded.kind, decoded.request_id, decoded.payload), (103, 42, payload));
        }

        let payload = [1, 2, 3];
        let mut encoded = Vec::new();
        write_frame(&mut encoded, 103, 7, &payload).unwrap();
        assert!(matches!(
            read_frame(&mut encoded.as_slice(), payload.len() - 1),
            Err(Error::MessageTooLarge)
        ));
    }

    #[test]
    fn every_truncated_frame_is_rejected() {
        let mut encoded = Vec::new();
        write_frame(&mut encoded, 103, 42, &[7; 32]).unwrap();

        for len in 0..encoded.len() {
            assert!(matches!(
                read_frame(&mut encoded[..len].as_ref(), MAX_CONTROL_BYTES),
                Err(Error::Io(_))
            ));
        }
    }

    #[test]
    fn request_and_response_frames_have_stable_wire_layouts() {
        let request = Request::SetCacheRecording(message::RecordingPolicy {
            enabled: true,
            capture_backtraces: false,
            sampling_one_in: 16,
        });
        let mut request_bytes = Vec::new();
        write_request(&mut request_bytes, 0x0102_0304_0506_0708, &request).unwrap();
        assert_eq!(
            request_bytes,
            [
                b'S', b'G', b'M', b'P', 9, 0, 5, 0, 8, 7, 6, 5, 4, 3, 2, 1, 6, 0, 0, 0, 1, 0, 16, 0, 0, 0,
            ]
        );

        let response = Response::CacheRecording(message::RecordingPolicy {
            enabled: false,
            capture_backtraces: true,
            sampling_one_in: 256,
        });
        let mut response_bytes = Vec::new();
        write_response(&mut response_bytes, 9, &response).unwrap();
        assert_eq!(
            response_bytes,
            [
                b'S', b'G', b'M', b'P', 9, 0, 105, 0, 9, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 1, 0, 1, 0, 0,
            ]
        );
    }

    #[test]
    fn snapshot_response_writes_the_original_payload_with_stable_framing() {
        struct SnapshotWriter {
            payload_address: *const u8,
            header: [u8; FRAME_HEADER_BYTES],
            written: usize,
            flushed: bool,
        }

        impl Write for SnapshotWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                let len = buf.len().min(3);
                if self.written < FRAME_HEADER_BYTES {
                    self.header[self.written..self.written + len].copy_from_slice(&buf[..len]);
                } else {
                    let offset = self.written - FRAME_HEADER_BYTES;
                    assert_eq!(buf.as_ptr(), self.payload_address.wrapping_add(offset));
                }
                self.written += len;
                Ok(len)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.flushed = true;
                Ok(())
            }
        }

        let payload = vec![0xA5; 17];
        let payload_len = payload.len();
        let mut writer = SnapshotWriter {
            payload_address: payload.as_ptr(),
            header: [0; FRAME_HEADER_BYTES],
            written: 0,
            flushed: false,
        };
        let response = Response::Snapshot(payload);
        write_response(&mut writer, 0x0102_0304_0506_0708, &response).unwrap();

        assert_eq!(
            (writer.header, writer.written, writer.flushed),
            (
                [b'S', b'G', b'M', b'P', 9, 0, 103, 0, 8, 7, 6, 5, 4, 3, 2, 1, 17, 0, 0, 0],
                FRAME_HEADER_BYTES + payload_len,
                true,
            )
        );
    }
    #[test]
    fn gamma_round2_message_lengths_preserve_the_exact_u32_limit_without_large_payloads() {
        assert_eq!(message_length(37).unwrap(), 37);
        assert_eq!(message_length(u32::MAX as usize).unwrap(), u32::MAX);
        if let Some(overflow) = (u32::MAX as usize).checked_add(1) {
            assert!(matches!(message_length(overflow), Err(Error::MessageTooLarge)));
        }
    }
}
