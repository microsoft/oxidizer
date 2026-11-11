// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashSet;

use super::{Reader, Writer, validate_count_fits};
use crate::Error;
use crate::recorder::event::{Address, Events};

const FIXED_LEN: usize = 28;
const NAME_PRESENT: u32 = 1;
const FILENAME_PRESENT: u32 = 2;
const LINE_PRESENT: u32 = 4;
const COLUMN_PRESENT: u32 = 8;
const ALL_FIELDS: u32 = NAME_PRESENT | FILENAME_PRESENT | LINE_PRESENT | COLUMN_PRESENT;

/// Symbol information for one recorded instruction address.
///
/// Resolution uses the producing process and its available debug information.
/// Missing fields remain unknown. Reading a file does not resolve addresses in
/// the reader's process or require the original executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Symbol {
    /// Recorded return address, unchanged from the corresponding event frames.
    pub address: Address,
    /// Resolved function name, when available.
    pub name: Option<String>,
    /// Resolved source filename, when available.
    pub filename: Option<String>,
    /// Source line number, when available.
    pub line: Option<u32>,
    /// Source column number, when available.
    pub column: Option<u32>,
}

pub(super) fn capture(events: &Events) -> Vec<Symbol> {
    capture_with(events, resolve)
}

pub(super) fn capture_with(events: &Events, mut resolve: impl FnMut(&mut Symbol)) -> Vec<Symbol> {
    let addresses = events
        .events
        .iter()
        .flat_map(|event| &event.call_stack)
        .copied()
        .filter(|address| address.get() != 0)
        .collect::<HashSet<_>>();
    let mut addresses = addresses.into_iter().collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses
        .into_iter()
        .map(|address| {
            let mut symbol = Symbol {
                address,
                name: None,
                filename: None,
                line: None,
                column: None,
            };
            resolve(&mut symbol);
            symbol
        })
        .collect()
}

#[cfg(not(miri))]
fn resolve(lookup: &mut Symbol) {
    let address = usize::try_from(crate::recorder::symbol_lookup_address(lookup.address).get())
        .expect("native frame addresses originate from pointer-width addresses in this process");
    backtrace::resolve(std::ptr::with_exposed_provenance_mut::<std::ffi::c_void>(address), |symbol| {
        lookup.name = lookup.name.take().or_else(|| symbol.name().map(|name| name.to_string()));
        lookup.filename = lookup
            .filename
            .take()
            .or_else(|| symbol.filename().map(|path| path.to_string_lossy().into_owned()));
        lookup.line = lookup.line.or_else(|| symbol.lineno());
        lookup.column = lookup.column.or_else(|| symbol.colno());
    });
}

#[cfg(miri)]
fn resolve(_lookup: &mut Symbol) {}

pub(super) fn encoded_len(symbols: &[Symbol]) -> Option<usize> {
    symbols.iter().try_fold(4_usize, |len, symbol| {
        len.checked_add(FIXED_LEN)?
            .checked_add(symbol.name.as_ref().map_or(0, String::len))?
            .checked_add(symbol.filename.as_ref().map_or(0, String::len))
    })
}

pub(super) fn encode(symbols: &[Symbol], writer: &mut Writer<'_>) -> Result<(), Error> {
    writer.u32(u32::try_from(symbols.len()).map_err(|_error| Error::invalid_format())?)?;
    let mut previous = 0;
    for symbol in symbols {
        if symbol.address.get() <= previous {
            return Err(Error::invalid_format());
        }
        previous = symbol.address.get();
        let fields = u32::from(symbol.name.is_some())
            | (u32::from(symbol.filename.is_some()) * FILENAME_PRESENT)
            | (u32::from(symbol.line.is_some()) * LINE_PRESENT)
            | (u32::from(symbol.column.is_some()) * COLUMN_PRESENT);
        writer.u64(symbol.address.get())?;
        writer.u32(fields)?;
        let name = symbol.name.as_deref().unwrap_or("");
        let filename = symbol.filename.as_deref().unwrap_or("");
        writer.u32(u32::try_from(name.len()).map_err(|_error| Error::invalid_format())?)?;
        writer.u32(u32::try_from(filename.len()).map_err(|_error| Error::invalid_format())?)?;
        writer.u32(symbol.line.unwrap_or(0))?;
        writer.u32(symbol.column.unwrap_or(0))?;
        writer.write(name.as_bytes())?;
        writer.write(filename.as_bytes())?;
    }
    Ok(())
}

pub(super) fn decode(reader: &mut Reader<'_>) -> Result<Vec<Symbol>, Error> {
    let count = reader.u32()? as usize;
    validate_count_fits(count, reader.remaining().len(), FIXED_LEN)?;
    let mut symbols = Vec::with_capacity(count);
    let mut previous = 0;
    for _ in 0..count {
        let address = reader.u64()?;
        let fields = reader.u32()?;
        let name_len = reader.u32()? as usize;
        let filename_len = reader.u32()? as usize;
        let line = reader.u32()?;
        let column = reader.u32()?;
        if address <= previous
            || fields & !ALL_FIELDS != 0
            || (fields & LINE_PRESENT == 0 && line != 0)
            || (fields & COLUMN_PRESENT == 0 && column != 0)
        {
            return Err(Error::invalid_format());
        }
        previous = address;
        symbols.push(Symbol {
            address: Address::new(address),
            name: decode_text(reader, fields & NAME_PRESENT != 0, name_len)?,
            filename: decode_text(reader, fields & FILENAME_PRESENT != 0, filename_len)?,
            line: (fields & LINE_PRESENT != 0).then_some(line),
            column: (fields & COLUMN_PRESENT != 0).then_some(column),
        });
    }
    Ok(symbols)
}

fn decode_text(reader: &mut Reader<'_>, present: bool, len: usize) -> Result<Option<String>, Error> {
    if !present {
        return if len == 0 { Ok(None) } else { Err(Error::invalid_format()) };
    }
    std::str::from_utf8(reader.read(len)?)
        .map(|text| Some(text.to_owned()))
        .map_err(|_error| Error::invalid_format())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::event::{Event, EventKind, EventPayload, EventSequence, EventTimestamp, ObjectId};
    use crate::recorder::thread::ThreadId;

    fn unresolved(address: u64) -> Symbol {
        Symbol {
            address: Address::new(address),
            name: None,
            filename: None,
            line: None,
            column: None,
        }
    }

    fn encoded(symbols: &[Symbol]) -> Vec<u8> {
        let mut bytes = vec![0; encoded_len(symbols).unwrap()];
        encode(symbols, &mut Writer::new(&mut bytes)).unwrap();
        bytes
    }

    #[test]
    fn capture_resolves_each_distinct_nonzero_frame_once_across_event_kinds() {
        let event = |kind, stack: &[u64]| Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(1),
            timestamp: EventTimestamp::from_ticks(1),
            kind,
            payload: EventPayload::Object(ObjectId::new(1)),
            call_stack: stack.iter().copied().map(Address::new).collect(),
        };
        let events = Events {
            events: vec![
                event(EventKind::ArcClone, &[32, 16, 0, 16]),
                event(EventKind::MutexAccess, &[16, 32]),
            ],
            ..Events::default()
        };
        let mut calls = Vec::new();

        let symbols = capture_with(&events, |lookup| {
            calls.push(lookup.address.get());
            if lookup.address == Address::new(16) {
                lookup.name = Some("application::allocate".into());
            }
        });

        assert_eq!(
            (calls, symbols),
            (
                vec![16, 32],
                vec![
                    Symbol {
                        name: Some("application::allocate".into()),
                        ..unresolved(16)
                    },
                    unresolved(32)
                ]
            )
        );
    }

    #[test]
    fn capture_without_nonzero_frames_does_not_invoke_the_resolver() {
        let symbols = capture_with(&Events::default(), |_| panic!("no frames require resolution"));

        assert_eq!(symbols, []);
    }

    #[test]
    fn symbol_codec_preserves_resolved_unresolved_and_empty_optional_fields() {
        let symbols = vec![
            Symbol {
                name: Some("application::allocate<&str>".into()),
                filename: Some("src\\main.rs".into()),
                line: Some(31),
                column: Some(7),
                ..unresolved(16)
            },
            unresolved(32),
            Symbol {
                name: Some(String::new()),
                filename: Some(String::new()),
                line: Some(0),
                column: Some(u32::MAX),
                ..unresolved(48)
            },
        ];
        let bytes = encoded(&symbols);
        let mut reader = Reader::new(&bytes);

        let decoded = decode(&mut reader).unwrap();

        assert_eq!((decoded, reader.remaining()), (symbols, [].as_slice()));
    }

    #[test]
    fn empty_symbol_table_round_trips() {
        let bytes = encoded(&[]);

        assert_eq!(decode(&mut Reader::new(&bytes)).unwrap(), []);
    }

    #[test]
    fn truncated_symbol_records_are_rejected() {
        let bytes = encoded(&[Symbol {
            name: Some("application".into()),
            filename: Some("main.rs".into()),
            ..unresolved(16)
        }]);

        for end in 0..bytes.len() {
            assert!(decode(&mut Reader::new(&bytes[..end])).is_err(), "accepted prefix length {end}");
        }
    }

    #[test]
    fn malformed_counts_flags_and_optional_values_are_rejected() {
        let bytes = encoded(&[Symbol {
            name: Some("n".into()),
            filename: Some("f".into()),
            line: Some(3),
            column: Some(5),
            ..unresolved(16)
        }]);
        let mutations = [
            (0, u32::MAX),
            (12, ALL_FIELDS | 16),
            (12, ALL_FIELDS & !NAME_PRESENT),
            (12, ALL_FIELDS & !FILENAME_PRESENT),
            (12, ALL_FIELDS & !LINE_PRESENT),
            (12, ALL_FIELDS & !COLUMN_PRESENT),
            (16, u32::MAX),
            (20, u32::MAX),
        ];

        for (offset, value) in mutations {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(decode(&mut Reader::new(&invalid)).is_err(), "accepted field {offset} = {value}");
        }
        for offset in [32, 33] {
            let mut invalid = bytes.clone();
            invalid[offset] = 0xff;
            assert!(decode(&mut Reader::new(&invalid)).is_err(), "accepted invalid UTF-8 at {offset}");
        }
    }

    #[test]
    fn zero_duplicate_and_unordered_addresses_are_rejected() {
        let bytes = encoded(&[unresolved(16), unresolved(32)]);

        for (offset, address) in [(4, 0_u64), (32, 16), (32, 8)] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 8].copy_from_slice(&address.to_le_bytes());
            assert!(
                decode(&mut Reader::new(&invalid)).is_err(),
                "accepted address {address} at {offset}"
            );
        }
        for symbols in [
            vec![unresolved(0)],
            vec![unresolved(16), unresolved(16)],
            vec![unresolved(32), unresolved(16)],
        ] {
            let mut bytes = vec![0; encoded_len(&symbols).unwrap()];
            assert!(encode(&symbols, &mut Writer::new(&mut bytes)).is_err());
        }
    }
}
