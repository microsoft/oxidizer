// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashSet;

use crate::codec::{Reader, Writer, error};
use crate::mimalloc::{CLASS_COUNT, ClassState, GlobalState, Observation, ObservationSource, Owner, State};
use crate::snapshot::{BackendState, Header, SCHEMA_VERSION, Snapshot};
use crate::{Error, ErrorKind, MAX_OWNERS};

const MAGIC: &[u8; 8] = b"RALLOCV4";
const SNMALLOC: u8 = 1;
const MIMALLOC: u8 = 2;
const COMMON_LEN: usize = 12 + 4 * 8 + 2;
const GLOBAL_LEN: usize = 9 * 8 + CLASS_COUNT * 8 + 1;
const OWNER_LEN: usize = 8 + 3;
const OBSERVATION_LEN: usize = 4 * 8 + 2 + CLASS_COUNT * 10 * 8;
const VALIDATION_CHUNK: usize = 1024;

fn snmalloc_snapshot(header: &Header, global: &crate::native::GlobalState) -> crate::native::Snapshot {
    crate::native::Snapshot {
        captured_nanos: header.captured_nanos,
        session_id: header.session_id,
        round: header.round,
        owner_count: header.owner_count,
        owners_complete: header.owners_complete,
        publication_enabled: header.publication_enabled,
        owners: Vec::new(),
        global: *global,
    }
}

/// Returns the exact output length without allocating.
///
/// # Errors
/// Rejects invalid inventory metadata and excessive counts.
pub fn encoded_len(snapshot: &Snapshot) -> Result<usize, Error> {
    match &snapshot.backend {
        BackendState::Snmalloc(state) => encoded_len_snmalloc(&snapshot.header, &state.global, &state.owners),
        BackendState::Mimalloc(state) => encoded_len_mimalloc(&snapshot.header, &state.global, &state.owners),
    }
}

/// Encodes either backend into an exactly sized output without allocating.
///
/// # Errors
/// Rejects invalid state and an incorrect output length.
pub fn encode(snapshot: &Snapshot, output: &mut [u8]) -> Result<(), Error> {
    match &snapshot.backend {
        BackendState::Snmalloc(state) => encode_snmalloc(&snapshot.header, &state.global, &state.owners, output),
        BackendState::Mimalloc(state) => encode_mimalloc(&snapshot.header, &state.global, &state.owners, output),
    }
}

/// Sizes a snmalloc payload from borrowed owner rows without allocating.
///
/// # Errors
/// Rejects invalid inventory metadata and excessive counts.
pub fn encoded_len_snmalloc(header: &Header, global: &crate::native::GlobalState, owners: &[crate::native::Owner]) -> Result<usize, Error> {
    crate::encoded_len_with_owners(&snmalloc_snapshot(header, global), owners)
}

/// Encodes snmalloc state with the shared header and backend tag.
///
/// # Errors
/// Rejects invalid inventory metadata and an incorrect output length.
pub fn encode_snmalloc(
    header: &Header,
    global: &crate::native::GlobalState,
    owners: &[crate::native::Owner],
    output: &mut [u8],
) -> Result<(), Error> {
    crate::encode_with_owners(&snmalloc_snapshot(header, global), owners, output)
}

fn validate_owner(owner: &Owner) -> Result<(), Error> {
    match (owner.source, &owner.observation) {
        (ObservationSource::Published, None) | (ObservationSource::Unobserved | ObservationSource::Unavailable, Some(_)) => {
            return Err(error(ErrorKind::Malformed));
        }
        _ => {}
    }
    if let Some(observation) = &owner.observation {
        for class in &observation.classes {
            if class.observed_pages > class.queued_pages
                || class.retired_pages > class.observed_pages
                || class.used_blocks > class.capacity_blocks
                || class.capacity_blocks > class.reserved_blocks
                || class.free_blocks > class.capacity_blocks - class.used_blocks
                || class.local_free_blocks > class.capacity_blocks - class.used_blocks - class.free_blocks
            {
                return Err(error(ErrorKind::Malformed));
            }
        }
    }
    Ok(())
}

fn validate_count(header: &Header, count: usize) -> Result<(), Error> {
    if count > MAX_OWNERS {
        return Err(error(ErrorKind::LengthOverflow));
    }
    if header.owner_count < count as u64 || header.owners_complete && header.owner_count != count as u64 {
        return Err(error(ErrorKind::Malformed));
    }
    Ok(())
}

fn validate(header: &Header, owners: &[Owner]) -> Result<(), Error> {
    validate_count(header, owners.len())?;
    // Bounded stack chunks avoid global allocator
    // activity while the producer holds System-backed inventory storage.
    for (index, chunk) in owners.chunks(VALIDATION_CHUNK).enumerate() {
        let mut scratch = [0; VALIDATION_CHUNK];
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

/// Sizes mimalloc state from borrowed rows without allocating.
///
/// # Errors
/// Rejects invalid inventories, class counts, and excessive payload sizes.
pub fn encoded_len_mimalloc(header: &Header, _global: &GlobalState, owners: &[Owner]) -> Result<usize, Error> {
    validate(header, owners)?;
    let observations = owners.iter().filter(|owner| owner.observation.is_some()).count();
    COMMON_LEN
        .checked_add(4 + GLOBAL_LEN)
        .and_then(|len| len.checked_add(owners.len().checked_mul(OWNER_LEN)?))
        .and_then(|len| len.checked_add(observations.checked_mul(OBSERVATION_LEN)?))
        .ok_or(error(ErrorKind::LengthOverflow))
}

/// Encodes borrowed mimalloc state without allocating.
///
/// # Errors
/// Rejects invalid state and an incorrect output length.
pub fn encode_mimalloc(header: &Header, global: &GlobalState, owners: &[Owner], output: &mut [u8]) -> Result<(), Error> {
    let len = encoded_len_mimalloc(header, global, owners)?;
    if output.len() != len {
        return Err(error(ErrorKind::LengthMismatch));
    }
    let mut writer = Writer { output, position: 0 };
    writer.bytes(MAGIC)?;
    writer.bytes(&SCHEMA_VERSION.to_le_bytes())?;
    writer.bytes(&[MIMALLOC, 0])?;
    for value in [header.captured_nanos, header.session_id, header.round, header.owner_count] {
        writer.u64(value)?;
    }
    writer.flag(header.owners_complete)?;
    writer.flag(header.publication_enabled)?;
    let count = u32::try_from(owners.len()).map_err(|_overflow| error(ErrorKind::LengthOverflow))?;
    writer.bytes(&count.to_le_bytes())?;
    for value in [
        global.arena_count,
        global.reserved_bytes,
        global.committed_bytes,
        global.free_slices,
        global.pending_purge_slices,
        global.metadata_bytes,
        global.os_allocations,
        global.os_reserved_bytes,
        global.os_committed_bytes,
    ] {
        writer.u64(value)?;
    }
    for count in global.abandoned_pages {
        writer.u64(count)?;
    }
    writer.flag(global.arenas_complete)?;
    for owner in owners {
        writer.u64(owner.id)?;
        writer.flag(owner.active)?;
        let source = match owner.source {
            ObservationSource::Unobserved => 0,
            ObservationSource::Published => 1,
            ObservationSource::Busy => 2,
            ObservationSource::Unavailable => 3,
        };
        writer.bytes(&[source])?;
        writer.flag(owner.observation.is_some())?;
        if let Some(observation) = &owner.observation {
            for value in [
                observation.session_id,
                observation.round,
                observation.thread_id,
                observation.captured_nanos,
            ] {
                writer.u64(value)?;
            }
            writer.flag(observation.pages_complete)?;
            writer.flag(observation.free_lists_complete)?;
            for class in &observation.classes {
                for value in [
                    class.block_bytes,
                    class.page_bytes,
                    class.queued_pages,
                    class.observed_pages,
                    class.reserved_blocks,
                    class.capacity_blocks,
                    class.used_blocks,
                    class.free_blocks,
                    class.local_free_blocks,
                    class.retired_pages,
                ] {
                    writer.u64(value)?;
                }
            }
        }
    }
    debug_assert_eq!(writer.position, len);
    Ok(())
}

/// Decodes the current allocator schema for either backend.
///
/// # Errors
/// Rejects unsupported tags, malformed fields, truncated or trailing bytes,
/// duplicate owner IDs, and forged counts before allocating owner storage.
pub fn decode(input: &[u8]) -> Result<Snapshot, Error> {
    let mut reader = Reader { input, position: 0 };
    if reader.bytes(8)? != MAGIC {
        return Err(error(ErrorKind::InvalidMagic));
    }
    let schema = u16::from_le_bytes(reader.bytes(2)?.try_into().map_err(|_length| error(ErrorKind::Truncated))?);
    if schema != SCHEMA_VERSION {
        return Err(error(ErrorKind::UnsupportedSchema(schema)));
    }
    let tag = reader.bytes(1)?[0];
    if reader.bytes(1)? != [0] {
        return Err(error(ErrorKind::Malformed));
    }
    if tag == SNMALLOC {
        return crate::decode(input).map(Snapshot::from);
    }
    if tag != MIMALLOC {
        return Err(error(ErrorKind::UnsupportedBackend(tag)));
    }
    let header = Header {
        captured_nanos: reader.u64()?,
        session_id: reader.u64()?,
        round: reader.u64()?,
        owner_count: reader.u64()?,
        owners_complete: reader.flag()?,
        publication_enabled: reader.flag()?,
    };
    let count = u32::from_le_bytes(reader.bytes(4)?.try_into().map_err(|_length| error(ErrorKind::Truncated))?) as usize;
    if count > MAX_OWNERS {
        return Err(error(ErrorKind::LengthOverflow));
    }
    let mut global = GlobalState {
        arena_count: reader.u64()?,
        reserved_bytes: reader.u64()?,
        committed_bytes: reader.u64()?,
        free_slices: reader.u64()?,
        pending_purge_slices: reader.u64()?,
        metadata_bytes: reader.u64()?,
        os_allocations: reader.u64()?,
        os_reserved_bytes: reader.u64()?,
        os_committed_bytes: reader.u64()?,
        ..GlobalState::EMPTY
    };
    for count in &mut global.abandoned_pages {
        *count = reader.u64()?;
    }
    global.arenas_complete = reader.flag()?;
    if count > (input.len() - reader.position) / OWNER_LEN {
        return Err(error(ErrorKind::Truncated));
    }
    validate_count(&header, count)?;
    let mut owners = Vec::new();
    let mut ids = HashSet::new();
    for _ in 0..count {
        let owner = read_owner(&mut reader)?;
        validate_owner(&owner)?;
        if owner.id == 0 || !ids.insert(owner.id) {
            return Err(error(ErrorKind::DuplicateOwner));
        }

        owners.push(owner);
    }
    if reader.position != input.len() {
        return Err(error(ErrorKind::LengthMismatch));
    }
    Ok(Snapshot {
        header,
        backend: BackendState::Mimalloc(State { owners, global }),
    })
}

fn read_owner(reader: &mut Reader<'_>) -> Result<Owner, Error> {
    let mut owner = Owner {
        id: reader.u64()?,
        active: reader.flag()?,
        source: match reader.bytes(1)?[0] {
            0 => ObservationSource::Unobserved,
            1 => ObservationSource::Published,
            2 => ObservationSource::Busy,
            3 => ObservationSource::Unavailable,
            _ => return Err(error(ErrorKind::Malformed)),
        },
        observation: None,
    };
    if reader.flag()? {
        let mut observation = Observation {
            session_id: reader.u64()?,
            round: reader.u64()?,
            thread_id: reader.u64()?,
            captured_nanos: reader.u64()?,
            pages_complete: reader.flag()?,
            free_lists_complete: reader.flag()?,
            ..Observation::EMPTY
        };
        for class in &mut observation.classes {
            *class = ClassState {
                block_bytes: reader.u64()?,
                page_bytes: reader.u64()?,
                queued_pages: reader.u64()?,
                observed_pages: reader.u64()?,
                reserved_blocks: reader.u64()?,
                capacity_blocks: reader.u64()?,
                used_blocks: reader.u64()?,
                free_blocks: reader.u64()?,
                local_free_blocks: reader.u64()?,
                retired_pages: reader.u64()?,
            };
        }
        owner.observation = Some(observation);
    }
    Ok(owner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_count_validation_bounds_storage_before_row_allocation() {
        let header = Header {
            owner_count: MAX_OWNERS as u64,
            owners_complete: true,
            ..Default::default()
        };

        assert_eq!(
            (
                validate_count(&header, MAX_OWNERS),
                validate_count(&header, MAX_OWNERS + 1).unwrap_err().kind(),
                validate_count(&header, MAX_OWNERS - 1).unwrap_err().kind(),
            ),
            (Ok(()), ErrorKind::LengthOverflow, ErrorKind::Malformed)
        );
    }
}
