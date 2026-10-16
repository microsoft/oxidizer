// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Monitor identities and secret-free discovery descriptors.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{fmt, io};

use crate::codec::{SliceReader, hex, push_string, push_u16, push_u32};
use crate::io::MonitorIo;
use crate::{Error, VERSION, platform};

const DESCRIPTOR_MAGIC: [u8; 4] = *b"SGMD";
// Three length-prefixed strings plus identity fields and a Windows SID.
const MAX_DESCRIPTOR_BYTES: usize = 3 * u16::MAX as usize + 1_024;

/// Identity assigned to one monitored process instance, independent of PID reuse.
///
/// This is an identifier, not an authentication secret.
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

/// OS identity that owns a monitor. Descriptor claims are checked against OS
/// ownership and the connected server, never used as credentials.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MonitorOwner {
    /// Effective Unix user ID of the monitor process.
    UnixUser(u32),
    /// Canonical Windows user SID, such as `S-1-5-21-1-2-3-1001`.
    WindowsSid(String),
}

impl MonitorOwner {
    /// Returns the caller's OS identity.
    ///
    /// # Errors
    ///
    /// Returns an error if the operating system cannot supply the identity.
    pub fn current() -> io::Result<Self> {
        platform::current_owner()
    }
}

impl fmt::Display for MonitorOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnixUser(uid) => uid.fmt(f),
            Self::WindowsSid(sid) => f.write_str(sid),
        }
    }
}

impl FromStr for MonitorOwner {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Ok(uid) = value.parse::<u32>()
            && uid.to_string() == value
        {
            return Ok(Self::UnixUser(uid));
        }
        let Some(sid) = value.strip_prefix("S-1-") else {
            return Err(Error::InvalidDescriptor);
        };
        let mut components = sid.split('-');
        let authority = components.next().ok_or(Error::InvalidDescriptor)?;
        let authority_value = authority.parse::<u64>().map_err(|_error| Error::InvalidDescriptor)?;
        if authority_value > 0xffff_ffff_ffff || authority_value.to_string() != authority {
            return Err(Error::InvalidDescriptor);
        }
        let mut count = 0;
        for component in components {
            let value = component.parse::<u32>().map_err(|_error| Error::InvalidDescriptor)?;
            if value.to_string() != component {
                return Err(Error::InvalidDescriptor);
            }
            count += 1;
        }
        if !(1..=15).contains(&count) {
            return Err(Error::InvalidDescriptor);
        }
        Ok(Self::WindowsSid(value.to_owned()))
    }
}

/// Local endpoint advertised by a monitor. There is no TCP transport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MonitorEndpoint {
    /// Absolute pathname of a Unix-domain stream socket.
    UnixSocket(PathBuf),
    /// Local Windows named-pipe path in the `\\.\pipe\` namespace.
    NamedPipe(PathBuf),
}

impl MonitorEndpoint {
    /// Constructs the endpoint for a process instance in its discovery directory.
    #[must_use]
    pub fn for_instance(directory: &Path, owner: &MonitorOwner, process_id: u32, instance_id: InstanceId) -> Self {
        let identity = format!("{process_id}-{}", instance_id.to_hex());
        match owner {
            MonitorOwner::UnixUser(_) => Self::UnixSocket(directory.join(format!("{identity}.sock"))),
            MonitorOwner::WindowsSid(sid) => Self::NamedPipe(PathBuf::from(format!(r"\\.\pipe\seismograph-{sid}-{identity}"))),
        }
    }

    /// Returns the native pathname.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::UnixSocket(path) | Self::NamedPipe(path) => path,
        }
    }
}

impl fmt::Display for MonitorEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path().display().fmt(f)
    }
}

/// Non-secret metadata published by a running monitor.
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
    /// Owning OS user, verified during discovery and connection.
    pub owner: MonitorOwner,
    /// OS-protected local endpoint.
    pub endpoint: MonitorEndpoint,
}

impl MonitorDescriptor {
    /// Returns the descriptor file name.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}-{}.monitor", self.process_id, self.instance_id.to_hex())
    }

    /// Encodes and exclusively creates a descriptor in its owner's private
    /// directory. Existing files and symbolic links are never overwritten.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid metadata, unsafe directory permissions,
    /// an occupied file name, or an I/O failure.
    pub fn write_file(&self, io: &MonitorIo, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let bytes = encode_descriptor(self)?;
        let mut file = io
            .create_descriptor(path, &self.owner)
            .map_err(|source| registry_error(path, source))?;
        let result = file.write_all(&bytes);
        drop(file);
        result.map_err(|source| io.cleanup_descriptor(path, registry_error(path, source)))
    }

    /// Reads a bounded descriptor without following symbolic links and verifies
    /// its OS owner, filename, and endpoint namespace.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe ownership, malformed or incompatible
    /// metadata, or an I/O failure.
    pub fn read_file(io: &MonitorIo, path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let (file, owner) = io.read_descriptor(path).map_err(|source| registry_error(path, source))?;
        let mut bytes = Vec::new();
        file.take(MAX_DESCRIPTOR_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| registry_error(path, source))?;
        if bytes.len() > MAX_DESCRIPTOR_BYTES {
            return Err(Error::MessageTooLarge);
        }
        let descriptor = decode_descriptor(&bytes)?;
        let expected_endpoint = MonitorEndpoint::for_instance(
            path.parent().ok_or(Error::InvalidDescriptor)?,
            &descriptor.owner,
            descriptor.process_id,
            descriptor.instance_id,
        );
        if descriptor.owner != owner
            || path.file_name() != Some(std::ffi::OsStr::new(&descriptor.file_name()))
            || descriptor.endpoint != expected_endpoint
        {
            return Err(Error::InvalidDescriptor);
        }
        Ok(descriptor)
    }

    pub(crate) fn endpoint_matches_identity(&self) -> bool {
        if self.process_id == 0
            || !self
                .owner
                .to_string()
                .parse::<MonitorOwner>()
                .as_ref()
                .is_ok_and(|owner| owner == &self.owner)
        {
            return false;
        }
        let directory = match &self.endpoint {
            MonitorEndpoint::UnixSocket(path) if path.to_str().is_some_and(|path| path.starts_with('/')) => {
                let Some(parent) = path.parent() else { return false };
                parent
            }
            MonitorEndpoint::NamedPipe(_) => Path::new(""),
            MonitorEndpoint::UnixSocket(_) => return false,
        };
        self.endpoint == MonitorEndpoint::for_instance(directory, &self.owner, self.process_id, self.instance_id)
    }
}

fn registry_error(path: &Path, source: io::Error) -> Error {
    Error::Registry {
        path: path.to_owned(),
        source,
    }
}

fn encode_descriptor(descriptor: &MonitorDescriptor) -> Result<Vec<u8>, Error> {
    if !descriptor.endpoint_matches_identity() || descriptor.name.is_empty() || descriptor.instance.as_ref().is_some_and(String::is_empty) {
        return Err(Error::InvalidDescriptor);
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&DESCRIPTOR_MAGIC);
    push_u16(&mut bytes, VERSION);
    push_u32(&mut bytes, descriptor.process_id);
    bytes.extend_from_slice(&descriptor.instance_id.as_bytes());
    match &descriptor.owner {
        MonitorOwner::UnixUser(uid) => {
            bytes.push(1);
            push_u32(&mut bytes, *uid);
        }
        MonitorOwner::WindowsSid(sid) => {
            bytes.push(2);
            push_string(&mut bytes, sid)?;
        }
    }
    bytes.push(match descriptor.endpoint {
        MonitorEndpoint::UnixSocket(_) => 1,
        MonitorEndpoint::NamedPipe(_) => 2,
    });
    push_string(&mut bytes, descriptor.endpoint.path().to_str().ok_or(Error::InvalidDescriptor)?)?;
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
    let mut instance_id = [0; 16];
    instance_id.copy_from_slice(reader.take(16)?);
    let owner = match reader.u8()? {
        1 => MonitorOwner::UnixUser(reader.u32()?),
        2 => match reader.string()?.parse()? {
            owner @ MonitorOwner::WindowsSid(_) => owner,
            MonitorOwner::UnixUser(_) => return Err(Error::InvalidDescriptor),
        },
        _ => return Err(Error::InvalidDescriptor),
    };
    let endpoint_kind = reader.u8()?;
    let path = PathBuf::from(reader.string()?);
    let endpoint = match endpoint_kind {
        1 => MonitorEndpoint::UnixSocket(path),
        2 => MonitorEndpoint::NamedPipe(path),
        _ => return Err(Error::InvalidDescriptor),
    };
    let name = reader.string()?;
    let instance = match reader.u8()? {
        0 => None,
        1 => Some(reader.string()?),
        _ => return Err(Error::InvalidDescriptor),
    };
    reader.finish()?;
    let descriptor = MonitorDescriptor {
        name,
        instance,
        process_id,
        instance_id: InstanceId::from_bytes(instance_id),
        owner,
        endpoint,
    };
    if !descriptor.endpoint_matches_identity() || descriptor.name.is_empty() || descriptor.instance.as_ref().is_some_and(String::is_empty) {
        return Err(Error::InvalidDescriptor);
    }
    Ok(descriptor)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn descriptor() -> MonitorDescriptor {
        let owner = MonitorOwner::UnixUser(1001);
        let instance_id = InstanceId::from_bytes([7; 16]);
        MonitorDescriptor {
            name: "orders".into(),
            instance: Some("shard-3".into()),
            process_id: 42,
            instance_id,
            endpoint: MonitorEndpoint::for_instance(Path::new("/run/user/1001/seismograph"), &owner, 42, instance_id),
            owner,
        }
    }

    #[test]
    fn descriptors_round_trip_without_secrets() {
        let mut descriptor = descriptor();
        for owner in [MonitorOwner::UnixUser(1001), "S-1-5-21-1-2-3-1001".parse().unwrap()] {
            descriptor.endpoint =
                MonitorEndpoint::for_instance(Path::new("/run/user/1001/seismograph"), &owner, 42, descriptor.instance_id);
            descriptor.owner = owner;
            for instance in [Some("shard-3".into()), None] {
                descriptor.instance = instance;
                let bytes = encode_descriptor(&descriptor).unwrap();
                assert_eq!(decode_descriptor(&bytes).unwrap(), descriptor);
                assert_eq!(&bytes[..10], &[b'S', b'G', b'M', b'D', 8, 0, 42, 0, 0, 0]);
            }
        }
    }

    #[test]
    fn malformed_truncated_and_old_descriptors_are_rejected() {
        let bytes = encode_descriptor(&descriptor()).unwrap();
        for len in 0..bytes.len() {
            decode_descriptor(&bytes[..len]).unwrap_err();
        }
        let mut old = bytes.clone();
        old[4..6].copy_from_slice(&7_u16.to_le_bytes());
        assert!(matches!(decode_descriptor(&old), Err(Error::UnsupportedVersion)));
        let mut invalid = bytes.clone();
        invalid[0] = 0;
        assert!(matches!(decode_descriptor(&invalid), Err(Error::InvalidDescriptor)));
        let mut invalid = bytes;
        invalid.push(0);
        assert!(matches!(decode_descriptor(&invalid), Err(Error::InvalidDescriptor)));
    }

    #[test]
    fn owner_parsing_rejects_noncanonical_and_injected_identities() {
        assert_eq!("1001".parse::<MonitorOwner>().unwrap(), MonitorOwner::UnixUser(1001));
        for owner in ["", "01001", "-1", "S-1-", "S-1-5", "S-1-5-01", "S-1-5-1)(A;;GA;;;WD)", "../1001"] {
            assert!(owner.parse::<MonitorOwner>().is_err(), "{owner}");
        }
    }

    #[test]
    fn endpoint_identity_is_not_taken_from_an_arbitrary_path() {
        let mut descriptor = descriptor();
        descriptor.endpoint = MonitorEndpoint::UnixSocket(PathBuf::from("/run/user/1001/seismograph/another.sock"));
        assert!(matches!(encode_descriptor(&descriptor), Err(Error::InvalidDescriptor)));
        descriptor.endpoint = MonitorEndpoint::NamedPipe(PathBuf::from(r"\\server\pipe\monitor"));
        assert!(matches!(encode_descriptor(&descriptor), Err(Error::InvalidDescriptor)));
    }

    #[test]
    #[cfg_attr(miri, ignore = "native and careful suites cover filesystem ownership checks")]
    fn file_persistence_checks_names_and_does_not_overwrite() {
        let directory = crate::testing::Directory::new();
        let descriptor = directory.descriptor();
        let path = directory.path().join(descriptor.file_name());
        descriptor.write_file(&crate::io::MonitorIo::new_native(), &path).unwrap();
        assert_eq!(
            MonitorDescriptor::read_file(&crate::io::MonitorIo::new_native(), &path).unwrap(),
            descriptor
        );
        assert!(descriptor.write_file(&crate::io::MonitorIo::new_native(), &path).is_err());
        let wrong_name = directory.path().join("another.monitor");
        std::fs::rename(&path, &wrong_name).unwrap();
        assert!(matches!(
            MonitorDescriptor::read_file(&crate::io::MonitorIo::new_native(), wrong_name),
            Err(Error::InvalidDescriptor)
        ));
    }
}
