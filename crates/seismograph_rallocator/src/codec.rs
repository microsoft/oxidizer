// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded, explicit little-endian native inventory format.

use std::collections::HashSet;

use crate::native::{ClassState, GlobalState, LocalState, Observation, ObservationSource, Owner, Ranges, RemoteState, Snapshot};

/// Maximum inventory rows admitted by the wire codec (about 240 MiB at the limit).
pub const MAX_OWNERS: usize = 65_536;
const MAGIC: &[u8; 8] = b"RALLOCV4";
const HEADER_LEN: usize = 12;
const RANGES_LEN: usize = 64 * 8 + 1;
const CLASS_LEN: usize = 6 * 8 + 1;
const OBSERVATION_LEN: usize = 5 * 8 + 1 + 44 * CLASS_LEN + 3 * RANGES_LEN + 8 + 9 * 8 + 1;
const OWNER_LEN: usize = 2 * 8 + 3;
const FIXED_LEN: usize = HEADER_LEN + 4 * 8 + 2 + 4 + 4 * 8 + RANGES_LEN;
// One normal producer capture fits in 8 KiB; larger inventories use more chunks.
const VALIDATION_CHUNK: usize = 1024;

/// Stable category of a native codec failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// The payload magic does not identify native v4 observations.
    InvalidMagic,
    /// The schema is not supported by the selected decoder.
    UnsupportedSchema(u16),
    /// The payload ended before all fields were available.
    Truncated,
    /// Extra input or output bytes were supplied.
    LengthMismatch,
    /// A count exceeds the bounded inventory limit or representable length.
    LengthOverflow,
    /// A boolean, enum, reserved field or inventory invariant is invalid.
    Malformed,
    /// An endpoint appears more than once.
    DuplicateOwner,
    /// A backend tag is not supported by this decoder.
    UnsupportedBackend(u8),
}

/// A native inventory encoding or decoding failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    kind: ErrorKind,
}

impl Error {
    /// Returns the stable failure category.
    #[must_use]
    pub const fn kind(self) -> ErrorKind {
        self.kind
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("native allocator payload: ")?;
        match self.kind {
            ErrorKind::InvalidMagic => f.write_str("the magic bytes are invalid"),
            ErrorKind::UnsupportedSchema(schema) => write!(f, "schema {schema} is unsupported by this decoder"),
            ErrorKind::Truncated => f.write_str("the payload ended before all fields were available"),
            ErrorKind::LengthMismatch => f.write_str("the buffer length does not match the encoded payload"),
            ErrorKind::LengthOverflow => f.write_str("the inventory count or payload length exceeds the supported limit"),
            ErrorKind::Malformed => f.write_str("a field or inventory invariant is invalid"),
            ErrorKind::DuplicateOwner => f.write_str("owner identifiers must be nonzero and unique"),
            ErrorKind::UnsupportedBackend(backend) => write!(f, "backend tag {backend} is unsupported"),
        }
    }
}
impl core::error::Error for Error {}

pub(crate) const fn error(kind: ErrorKind) -> Error {
    Error { kind }
}

fn validate(snapshot: &Snapshot, owners: &[Owner]) -> Result<(), Error> {
    if owners.len() > MAX_OWNERS {
        return Err(error(ErrorKind::LengthOverflow));
    }
    if snapshot.owner_count < owners.len() as u64 || (snapshot.owners_complete && snapshot.owner_count != owners.len() as u64) {
        return Err(error(ErrorKind::Malformed));
    }
    for (index, chunk) in owners.chunks(VALIDATION_CHUNK).enumerate() {
        // #[gamma::skip(literal.int_increment, reason = "the used scratch prefix is overwritten by owner IDs before sorting; the unused suffix is never read")]
        let mut scratch = [0_u64; VALIDATION_CHUNK];
        let ids = &mut scratch[..chunk.len()];
        for (id, owner) in ids.iter_mut().zip(chunk) {
            if owner.id == 0 {
                return Err(error(ErrorKind::DuplicateOwner));
            }
            validate_owner(owner)?;
            *id = owner.id;
        }
        ids.sort_unstable();
        if ids.windows(2).any(|pair| pair[0] == pair[1])
            || owners[..index * VALIDATION_CHUNK]
                .iter()
                .any(|owner| ids.binary_search(&owner.id).is_ok())
        {
            return Err(error(ErrorKind::DuplicateOwner));
        }
    }
    Ok(())
}

fn validate_owner(owner: &Owner) -> Result<(), Error> {
    match (owner.source, owner.observation.as_ref()) {
        (ObservationSource::Unobserved | ObservationSource::Unavailable, Some(_))
        | (ObservationSource::Published | ObservationSource::IdleInspection, None) => Err(error(ErrorKind::Malformed)),
        (ObservationSource::IdleInspection, Some(observation))
            if owner.leased || observation.generation != owner.generation || observation.session_id != 0 || observation.thread_id != 0 =>
        {
            Err(error(ErrorKind::Malformed))
        }
        _ => Ok(()),
    }
}

/// Exact current-schema snmalloc output length.
///
/// # Errors
/// Rejects inventories beyond the bound and invalid owner metadata.
pub fn encoded_len(snapshot: &Snapshot) -> Result<usize, Error> {
    encoded_len_with_owners(snapshot, &snapshot.owners)
}

/// Exact snmalloc length using borrowed inventory rows instead of `snapshot.owners`.
///
/// Does not allocate or deallocate through any allocator. Duplicate validation
/// sorts fixed 8 KiB stack chunks without modifying the supplied inventory.
/// `snapshot.owner_count` and `owners_complete` describe this supplied slice;
/// `snapshot.owners` is ignored and may be an unallocated empty vector.
///
/// # Errors
/// Rejects inventories beyond the bound and invalid owner metadata.
pub fn encoded_len_with_owners(snapshot: &Snapshot, owners: &[Owner]) -> Result<usize, Error> {
    validate(snapshot, owners)?;
    let observations = owners.iter().filter(|owner| owner.observation.is_some()).count();
    // #[gamma::skip(try.propagate_to_unwrap, reason = "validate bounds owners by MAX_OWNERS and observations cannot exceed owners; their fixed record sizes fit usize even on 32-bit targets")]
    FIXED_LEN
        .checked_add(owners.len().checked_mul(OWNER_LEN).ok_or(error(ErrorKind::LengthOverflow))?)
        .and_then(|len| len.checked_add(observations.checked_mul(OBSERVATION_LEN)?))
        .ok_or(error(ErrorKind::LengthOverflow))
}

/// Encodes a native inventory into an exactly sized output.
///
/// Does not allocate. Never serialize while borrowing a native owner core.
///
/// # Errors
/// Rejects invalid inventories and buffers differing from [`encoded_len`].
pub fn encode(snapshot: &Snapshot, output: &mut [u8]) -> Result<usize, Error> {
    encode_with_owners(snapshot, &snapshot.owners, output)?;
    Ok(output.len())
}

/// Encodes metadata and borrowed inventory rows without allocating.
///
/// Ignores `snapshot.owners`; all inventory validation uses `owners`, as in
/// [`encoded_len_with_owners`]. The caller may keep rows and output System-backed
/// so collecting telemetry never changes native allocator observations.
/// This function does not allocate or deallocate through any allocator.
/// Never serialize while borrowing a native owner core.
///
/// # Errors
/// Rejects invalid inventories and output lengths differing from
/// [`encoded_len_with_owners`].
pub fn encode_with_owners(snapshot: &Snapshot, owners: &[Owner], output: &mut [u8]) -> Result<(), Error> {
    let len = encoded_len_with_owners(snapshot, owners)?;
    if output.len() != len {
        return Err(error(ErrorKind::LengthMismatch));
    }
    let mut writer = Writer { output, position: 0 };
    // #[gamma::skip(try.propagate_to_unwrap, reason = "encoded_len_with_owners validated the inventory and the exact output length before these fixed-size writes")]
    writer.bytes(MAGIC)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length accounts for this schema field")]
    writer.bytes(&crate::snapshot::SCHEMA_VERSION.to_le_bytes())?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length accounts for this backend field")]
    writer.bytes(&[1, 0])?;
    for value in [snapshot.captured_nanos, snapshot.session_id, snapshot.round, snapshot.owner_count] {
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length accounts for every fixed header counter")]
        writer.u64(value)?;
    }
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length accounts for this flag")]
    writer.flag(snapshot.owners_complete)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length accounts for this flag")]
    writer.flag(snapshot.publication_enabled)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "validate bounds the count below u32::MAX and the exact output length accounts for its field")]
    writer.owner_count(owners.len())?;
    for value in [
        snapshot.global.reserved_bytes,
        snapshot.global.pagemap_reserved_bytes,
        snapshot.global.local_limit_bytes,
        snapshot.global.global_refill_bytes,
    ] {
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length accounts for every fixed global counter")]
        writer.u64(value)?;
    }
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length includes the fixed range histogram")]
    writer.ranges(&snapshot.global.ranges)?;
    for owner in owners {
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length includes each fixed owner record")]
        writer.u64(owner.id)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length includes each fixed owner record")]
        writer.u64(owner.generation)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length includes each fixed owner record")]
        writer.flag(owner.leased)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length includes each fixed owner record")]
        writer.source(owner.source)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded output length includes each fixed owner record")]
        writer.flag(owner.observation.is_some())?;
        if let Some(observation) = &owner.observation {
            // #[gamma::skip(try.propagate_to_unwrap, reason = "encoded_len_with_owners counted exactly the observations serialized by this loop")]
            writer.observation(observation)?;
        }
    }
    writer.finish(len)
}

pub(crate) struct Writer<'a> {
    pub(crate) output: &'a mut [u8],
    pub(crate) position: usize,
}
impl Writer<'_> {
    fn owner_count(&mut self, count: usize) -> Result<(), Error> {
        let count = u32::try_from(count).map_err(|_overflow| error(ErrorKind::LengthOverflow))?;
        self.bytes(&count.to_le_bytes())
    }
    fn source(&mut self, source: ObservationSource) -> Result<(), Error> {
        let value = match source {
            ObservationSource::Unobserved => 0,
            ObservationSource::Published => 1,
            ObservationSource::IdleInspection => 2,
            ObservationSource::Busy => 3,
            ObservationSource::Unavailable => 4,
        };
        self.bytes(&[value])
    }
    fn finish(self, len: usize) -> Result<(), Error> {
        if self.position != len {
            return Err(error(ErrorKind::LengthMismatch));
        }
        Ok(())
    }
    pub(crate) fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let end = self.position.checked_add(bytes.len()).ok_or(error(ErrorKind::LengthOverflow))?;
        self.output
            .get_mut(self.position..end)
            .ok_or(error(ErrorKind::Truncated))?
            .copy_from_slice(bytes);
        self.position = end;
        Ok(())
    }
    pub(crate) fn u64(&mut self, value: u64) -> Result<(), Error> {
        self.bytes(&value.to_le_bytes())
    }
    pub(crate) fn flag(&mut self, value: bool) -> Result<(), Error> {
        self.bytes(&[u8::from(value)])
    }
    fn ranges(&mut self, ranges: &Ranges) -> Result<(), Error> {
        for count in ranges.counts {
            self.u64(count)?;
        }
        self.flag(ranges.complete)
    }
    fn observation(&mut self, observation: &Observation) -> Result<(), Error> {
        for value in [
            observation.generation,
            observation.session_id,
            observation.round,
            observation.thread_id,
            observation.captured_nanos,
        ] {
            self.u64(value)?;
        }
        self.flag(observation.slabs_complete)?;
        for class in &observation.classes {
            for value in [
                class.object_bytes,
                class.slab_bytes,
                class.capacity,
                class.available_slabs,
                class.empty_slabs,
                class.observed_slabs,
            ] {
                self.u64(value)?;
            }
            self.flag(class.fast_nonempty)?;
        }
        self.ranges(&observation.large)?;
        self.ranges(&observation.local.ranges)?;
        self.ranges(&observation.local.metadata)?;
        self.u64(observation.local.requested_bytes)?;
        let remote = &observation.remote;
        for value in [
            remote.open_rings,
            remote.open_objects,
            remote.outgoing_lists,
            remote.messages,
            remote.message_objects,
            remote.message_bytes,
            remote.budget_remaining,
            remote.incoming_front,
            remote.incoming_back,
        ] {
            self.u64(value)?;
        }
        self.flag(remote.complete)
    }
}

pub(crate) struct Reader<'a> {
    pub(crate) input: &'a [u8],
    pub(crate) position: usize,
}
impl<'a> Reader<'a> {
    pub(crate) fn bytes(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.position.checked_add(len).ok_or(error(ErrorKind::LengthOverflow))?;
        let result = self.input.get(self.position..end).ok_or(error(ErrorKind::Truncated))?;
        self.position = end;
        Ok(result)
    }
    pub(crate) fn u64(&mut self) -> Result<u64, Error> {
        let bytes = self.bytes(8)?;
        Ok(u64::from_le_bytes(
            // #[gamma::skip(try.propagate_to_unwrap, reason = "the preceding bytes(8) either returns an exact eight-byte slice or propagates its truncation error before this conversion")]
            bytes.try_into().map_err(|_length| error(ErrorKind::Truncated))?,
        ))
    }
    pub(crate) fn flag(&mut self) -> Result<bool, Error> {
        match self.bytes(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(error(ErrorKind::Malformed)),
        }
    }
    fn ranges(&mut self) -> Result<Ranges, Error> {
        let mut ranges = Ranges::EMPTY;
        for count in &mut ranges.counts {
            *count = self.u64()?;
        }
        ranges.complete = self.flag()?;
        Ok(ranges)
    }
    fn observation(&mut self) -> Result<Observation, Error> {
        let mut observation = Observation {
            generation: self.u64()?,
            session_id: self.u64()?,
            round: self.u64()?,
            thread_id: self.u64()?,
            captured_nanos: self.u64()?,
            slabs_complete: self.flag()?,
            ..Observation::EMPTY
        };
        for class in &mut observation.classes {
            *class = ClassState {
                object_bytes: self.u64()?,
                slab_bytes: self.u64()?,
                capacity: self.u64()?,
                available_slabs: self.u64()?,
                empty_slabs: self.u64()?,
                observed_slabs: self.u64()?,
                fast_nonempty: self.flag()?,
            };
        }
        observation.large = self.ranges()?;
        observation.local = LocalState {
            ranges: self.ranges()?,
            metadata: self.ranges()?,
            requested_bytes: self.u64()?,
        };
        observation.remote = RemoteState {
            open_rings: self.u64()?,
            open_objects: self.u64()?,
            outgoing_lists: self.u64()?,
            messages: self.u64()?,
            message_objects: self.u64()?,
            message_bytes: self.u64()?,
            budget_remaining: self.u64()?,
            incoming_front: self.u64()?,
            incoming_back: self.u64()?,
            complete: self.flag()?,
        };
        Ok(observation)
    }
}

/// Decodes a bounded snmalloc inventory from the current schema.
///
/// # Errors
/// Reports malformed, truncated, duplicate, oversized and trailing input.
pub fn decode(input: &[u8]) -> Result<Snapshot, Error> {
    let mut reader = Reader { input, position: 0 };
    if reader.bytes(8)? != MAGIC {
        return Err(error(ErrorKind::InvalidMagic));
    }
    let schema_bytes = reader.bytes(2)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "Reader::bytes(2) returned an exact two-byte slice; its independently fallible read is not guarded")]
    let schema = u16::from_le_bytes(schema_bytes.try_into().map_err(|_length| error(ErrorKind::Truncated))?);
    if schema != crate::snapshot::SCHEMA_VERSION {
        return Err(error(ErrorKind::UnsupportedSchema(schema)));
    }
    if reader.bytes(2)? != [1, 0] {
        return Err(error(ErrorKind::Malformed));
    }
    let mut snapshot = Snapshot {
        captured_nanos: reader.u64()?,
        session_id: reader.u64()?,
        round: reader.u64()?,
        owner_count: reader.u64()?,
        owners_complete: reader.flag()?,
        publication_enabled: reader.flag()?,
        ..Snapshot::default()
    };
    let count_bytes = reader.bytes(4)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "Reader::bytes(4) returned an exact four-byte slice; its independently fallible read is not guarded")]
    let count = u32::from_le_bytes(count_bytes.try_into().map_err(|_length| error(ErrorKind::Truncated))?) as usize;
    if count > MAX_OWNERS {
        return Err(error(ErrorKind::LengthOverflow));
    }
    snapshot.global = GlobalState {
        reserved_bytes: reader.u64()?,
        pagemap_reserved_bytes: reader.u64()?,
        local_limit_bytes: reader.u64()?,
        global_refill_bytes: reader.u64()?,
        ranges: reader.ranges()?,
    };
    if count > (input.len() - reader.position) / OWNER_LEN {
        return Err(error(ErrorKind::Truncated));
    }
    if snapshot.owner_count < count as u64 || (snapshot.owners_complete && snapshot.owner_count != count as u64) {
        return Err(error(ErrorKind::Malformed));
    }
    let mut ids = HashSet::new();
    // Grow only after successfully decoding each row, not by a forged count.
    for _ in 0..count {
        let mut owner = Owner {
            id: reader.u64()?,
            generation: reader.u64()?,
            leased: reader.flag()?,
            source: match reader.bytes(1)?[0] {
                0 => ObservationSource::Unobserved,
                1 => ObservationSource::Published,
                2 => ObservationSource::IdleInspection,
                3 => ObservationSource::Busy,
                4 => ObservationSource::Unavailable,
                _ => return Err(error(ErrorKind::Malformed)),
            },
            observation: None,
        };
        if reader.flag()? {
            owner.observation = Some(reader.observation()?);
        }
        validate_owner(&owner)?;
        if owner.id == 0 || !ids.insert(owner.id) {
            return Err(error(ErrorKind::DuplicateOwner));
        }
        snapshot.owners.push(owner);
    }
    if reader.position != input.len() {
        return Err(error(ErrorKind::LengthMismatch));
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_round2_observation_writers_return_truncation_at_each_field() {
        for length in 0..OBSERVATION_LEN {
            let mut output = vec![0xa5; length];
            let mut writer = Writer {
                output: &mut output,
                position: 0,
            };
            assert_eq!(
                writer.observation(&Observation::EMPTY).unwrap_err().kind(),
                ErrorKind::Truncated,
                "observation {length}"
            );
        }
        for length in 0..RANGES_LEN {
            let mut output = vec![0xa5; length];
            let mut writer = Writer {
                output: &mut output,
                position: 0,
            };
            assert_eq!(
                writer.ranges(&Ranges::EMPTY).unwrap_err().kind(),
                ErrorKind::Truncated,
                "ranges {length}"
            );
        }
    }

    #[test]
    fn gamma_round2_reader_length_overflow_is_an_error_not_a_panic() {
        let mut reader = Reader { input: &[1], position: 1 };
        assert_eq!(reader.bytes(usize::MAX).unwrap_err().kind(), ErrorKind::LengthOverflow);
        assert_eq!(reader.position, 1);
        assert_eq!(reader.bytes(1).unwrap_err().kind(), ErrorKind::Truncated);
        assert_eq!(reader.position, 1);
    }

    #[test]
    fn inventory_fields_reject_overflow_and_truncation_without_advancing() {
        let mut output = [0xa5; 3];
        let mut writer = Writer {
            output: &mut output,
            position: 0,
        };
        assert_eq!(writer.owner_count(usize::MAX).unwrap_err().kind(), ErrorKind::LengthOverflow);
        assert_eq!(writer.owner_count(1).unwrap_err().kind(), ErrorKind::Truncated);
        assert_eq!(writer.position, 0);
        writer.source(ObservationSource::Busy).unwrap();
        assert_eq!(writer.finish(2).unwrap_err().kind(), ErrorKind::LengthMismatch);
        assert_eq!(output, [3, 0xa5, 0xa5]);

        let mut empty = [];
        let mut writer = Writer {
            output: &mut empty,
            position: 0,
        };
        assert_eq!(
            writer.source(ObservationSource::Unobserved).unwrap_err().kind(),
            ErrorKind::Truncated
        );
        writer.finish(0).unwrap();

        let mut output = [0; 4];
        let mut writer = Writer {
            output: &mut output,
            position: 0,
        };
        writer.owner_count(u32::MAX as usize).unwrap();
        writer.finish(4).unwrap();
        assert_eq!(output, u32::MAX.to_le_bytes());
    }

    #[test]
    fn writer_rejects_truncated_and_overflowing_positions_without_writing() {
        let mut output = [0xa5; 1];
        let mut writer = Writer {
            output: &mut output,
            position: 0,
        };
        assert_eq!(writer.bytes(&[1, 2]).unwrap_err().kind(), ErrorKind::Truncated);
        assert_eq!(writer.position, 0);
        writer.position = usize::MAX;
        assert_eq!(writer.bytes(&[1]).unwrap_err().kind(), ErrorKind::LengthOverflow);
        assert_eq!(output, [0xa5]);
    }
}
