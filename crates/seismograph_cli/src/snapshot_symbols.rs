// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeMap;

use seismograph_rallocator::callers::{AddressLookup, AddressLookupFields};

pub(crate) fn merge(
    allocator: Vec<AddressLookup>,
    runtime: Vec<seismograph_runtime::snapshot::AddressLookup>,
    shared: Vec<seismograph::snapshot::Symbol>,
) -> Vec<AddressLookup> {
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
    let runtime = runtime.into_iter().map(|lookup| AddressLookupFields {
        address: lookup.address,
        symbol: lookup.symbol,
        filename: lookup.filename,
        line: lookup.line,
        column: lookup.column,
    });
    // Source-specific names take precedence. Shared capture symbols fill missing fields.
    for lookup in allocator.into_iter().chain(runtime.map(AddressLookup::from_fields)) {
        if let Some(previous) = lookups.get_mut(&lookup.address) {
            previous.symbol = lookup.symbol.or_else(|| previous.symbol.take());
            previous.filename = lookup.filename.or_else(|| previous.filename.take());
            previous.line = lookup.line.or(previous.line);
            previous.column = lookup.column.or(previous.column);
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
    use seismograph_runtime::snapshot::AddressLookup as RuntimeLookup;

    use super::*;

    #[test]
    fn source_specific_symbols_take_precedence_without_erasing_available_common_fields() {
        let allocator = AddressLookup::from_fields(AddressLookupFields {
            address: 16,
            symbol: Some("allocator::name".into()),
            filename: Some("allocator.rs".into()),
            line: Some(5),
            column: None,
        });
        let runtime = RuntimeLookup {
            address: 16,
            symbol: Some("runtime::name".into()),
            filename: None,
            line: Some(7),
            column: None,
        };
        let shared = Symbol {
            address: Address::new(16),
            name: Some("application::allocate".into()),
            filename: None,
            line: None,
            column: Some(9),
        };

        let lookups = merge(vec![allocator], vec![runtime], vec![shared]);

        assert_eq!(
            lookups,
            vec![AddressLookup::from_fields(AddressLookupFields {
                address: 16,
                symbol: Some("runtime::name".into()),
                filename: Some("allocator.rs".into()),
                line: Some(7),
                column: Some(9),
            })]
        );
    }

    #[test]
    fn shared_only_and_runtime_only_addresses_remain_available_in_address_order() {
        let runtime = RuntimeLookup {
            address: 32,
            symbol: Some("runtime::spawn".into()),
            ..RuntimeLookup::default()
        };
        let shared = Symbol {
            address: Address::new(16),
            name: None,
            filename: Some("main.rs".into()),
            line: Some(3),
            column: None,
        };

        let lookups = merge(vec![], vec![runtime], vec![shared]);

        assert_eq!(
            lookups,
            vec![
                AddressLookup::from_fields(AddressLookupFields {
                    address: 16,
                    symbol: None,
                    filename: Some("main.rs".into()),
                    line: Some(3),
                    column: None,
                }),
                AddressLookup::from_fields(AddressLookupFields {
                    address: 32,
                    symbol: Some("runtime::spawn".into()),
                    filename: None,
                    line: None,
                    column: None,
                }),
            ]
        );
    }
}
