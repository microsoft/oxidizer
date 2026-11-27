// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeMap;

use seismograph_rallocator::callers::{AddressLookup, AddressLookupFields};

pub(crate) fn merge(allocator: Vec<AddressLookup>, shared: Vec<seismograph::snapshot::Symbol>) -> Vec<AddressLookup> {
    let mut lookups = shared
        .into_iter()
        .map(|lookup| {
            let fields = AddressLookupFields {
                address: lookup.address.get(),
                symbol: lookup.name,
                filename: lookup.filename,
                line: lookup.line,
                column: lookup.column,
            };
            (fields.address, AddressLookup::from_fields(fields))
        })
        .collect::<BTreeMap<_, _>>();
    // Source-specific names take precedence. Keep each location from one lookup.
    for lookup in allocator {
        if let Some(previous) = lookups.get_mut(&lookup.address) {
            previous.symbol = lookup.symbol.or_else(|| previous.symbol.take());
            if lookup.filename.is_some() {
                previous.filename = lookup.filename;
                previous.line = lookup.line;
                previous.column = lookup.column;
            }
        } else {
            lookups.insert(lookup.address, lookup);
        }
    }
    lookups.into_values().collect()
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::Address;
    use seismograph::snapshot::Symbol;

    use super::*;

    #[test]
    fn a_source_filename_without_coordinates_clears_the_previous_coordinates() {
        let allocator = AddressLookup::from_fields(AddressLookupFields {
            address: 16,
            symbol: Some("allocator::name".into()),
            filename: Some("new.rs".into()),
            line: None,
            column: None,
        });
        let shared = Symbol {
            address: Address::new(16),
            name: Some("application::allocate".into()),
            filename: Some("old.rs".into()),
            line: Some(100),
            column: Some(4),
        };

        assert_eq!(
            merge(vec![allocator], vec![shared]),
            vec![AddressLookup::from_fields(AddressLookupFields {
                address: 16,
                symbol: Some("allocator::name".into()),
                filename: Some("new.rs".into()),
                line: None,
                column: None,
            })]
        );
    }
}
