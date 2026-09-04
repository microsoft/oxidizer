// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Monitor identity, discovery, and descriptor persistence.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::{fmt, fs};

use crate::codec::{SliceReader, hex, push_string, push_u16, push_u32};
use crate::{Error, VERSION};

const DESCRIPTOR_MAGIC: [u8; 4] = *b"SGMD";

/// Stable identity assigned to one monitored process instance.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InstanceId([u8; 16]);

impl InstanceId {
    /// Constructs an identity from random bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Returns the identity bytes.
    #[must_use]
    pub const fn as_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Returns a lowercase hexadecimal representation.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex(&self.0)
    }
}

/// Secret required to connect to one monitor.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AuthenticationToken([u8; 32]);

impl AuthenticationToken {
    /// Constructs a token from random bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the token bytes.
    #[must_use]
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for AuthenticationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthenticationToken([REDACTED])")
    }
}

/// Metadata published by a running monitor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitorDescriptor {
    /// Human-readable application name.
    pub name: String,
    /// Optional application-provided instance label.
    pub instance: Option<String>,
    /// Operating-system process identifier.
    pub process_id: u32,
    /// Identity protecting against process-ID reuse.
    pub instance_id: InstanceId,
    /// Localhost port accepting monitor connections.
    pub port: u16,
    /// Secret required during the protocol handshake.
    pub authentication: AuthenticationToken,
}

impl MonitorDescriptor {
    /// Returns the localhost socket address advertised by this monitor.
    #[must_use]
    pub const fn socket_address(&self) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port)
    }

    /// Returns the descriptor file name.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}-{}.monitor", self.process_id, self.instance_id.to_hex())
    }

    /// Encodes and writes this descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error when the descriptor is too large or cannot be written.
    pub fn write_file(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let bytes = encode_descriptor(self)?;
        fs::write(path, bytes).map_err(Error::Io)
    }

    /// Reads and decodes a descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or is malformed.
    pub fn read_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let bytes = fs::read(path).map_err(Error::Io)?;
        decode_descriptor(&bytes)
    }
}

fn encode_descriptor(descriptor: &MonitorDescriptor) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&DESCRIPTOR_MAGIC);
    push_u16(&mut bytes, VERSION);
    push_u32(&mut bytes, descriptor.process_id);
    push_u16(&mut bytes, descriptor.port);
    bytes.extend_from_slice(&descriptor.instance_id.as_bytes());
    bytes.extend_from_slice(&descriptor.authentication.as_bytes());
    push_string(&mut bytes, &descriptor.name)?;
    match &descriptor.instance {
        Some(instance) => {
            bytes.push(1);
            push_string(&mut bytes, instance)?;
        }
        None => bytes.push(0),
    }
    Ok(bytes)
}

fn decode_descriptor(bytes: &[u8]) -> Result<MonitorDescriptor, Error> {
    let mut reader = SliceReader::new(bytes);
    if reader.take(4)? != DESCRIPTOR_MAGIC {
        return Err(Error::InvalidDescriptor);
    }
    if reader.u16()? != VERSION {
        return Err(Error::UnsupportedVersion);
    }
    let process_id = reader.u32()?;
    let port = reader.u16()?;
    let mut instance_id = [0; 16];
    instance_id.copy_from_slice(reader.take(16)?);
    let mut authentication = [0; 32];
    authentication.copy_from_slice(reader.take(32)?);
    let name = reader.string()?;
    let instance = match reader.u8()? {
        0 => None,
        1 => Some(reader.string()?),
        _ => return Err(Error::InvalidDescriptor),
    };
    reader.finish()?;
    Ok(MonitorDescriptor {
        name,
        instance,
        process_id,
        instance_id: InstanceId::from_bytes(instance_id),
        port,
        authentication: AuthenticationToken::from_bytes(authentication),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> MonitorDescriptor {
        MonitorDescriptor {
            name: "orders".into(),
            instance: Some("shard-3".into()),
            process_id: 42,
            instance_id: InstanceId::from_bytes([7; 16]),
            port: 7331,
            authentication: AuthenticationToken::from_bytes([9; 32]),
        }
    }

    #[test]
    fn descriptor_round_trips() {
        let descriptor = descriptor();
        assert_eq!(decode_descriptor(&encode_descriptor(&descriptor).unwrap()).unwrap(), descriptor);
    }

    #[test]
    fn authentication_debug_is_redacted() {
        assert_eq!(
            format!("{:?}", AuthenticationToken::from_bytes([0x42; 32])),
            "AuthenticationToken([REDACTED])"
        );
    }

    #[test]
    fn descriptor_helpers_and_file_persistence_work() {
        let descriptor = descriptor();
        assert_eq!(descriptor.socket_address(), SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7331));
        assert_eq!(descriptor.file_name(), "42-07070707070707070707070707070707.monitor");

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join(format!("seismograph-protocol-{}.monitor", std::process::id()));
        descriptor.write_file(&path).unwrap();
        let decoded = MonitorDescriptor::read_file(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn descriptor_without_instance_round_trips() {
        let mut descriptor = descriptor();
        descriptor.instance = None;
        assert_eq!(decode_descriptor(&encode_descriptor(&descriptor).unwrap()).unwrap(), descriptor);
    }

    #[test]
    fn malformed_descriptors_are_rejected() {
        let bytes = encode_descriptor(&descriptor()).unwrap();

        let mut invalid_magic = bytes.clone();
        invalid_magic[0] ^= 1;
        assert!(matches!(decode_descriptor(&invalid_magic), Err(Error::InvalidDescriptor)));

        let mut unsupported_version = bytes.clone();
        unsupported_version[4..6].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(matches!(decode_descriptor(&unsupported_version), Err(Error::UnsupportedVersion)));

        let instance_tag = 4 + 2 + 4 + 2 + 16 + 32 + 2 + descriptor().name.len();
        let mut invalid_instance_tag = bytes;
        invalid_instance_tag[instance_tag] = 2;
        assert!(matches!(decode_descriptor(&invalid_instance_tag), Err(Error::InvalidDescriptor)));
    }
}
