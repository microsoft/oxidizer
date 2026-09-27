// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;
use std::sync::Arc;

use seismograph::recorder::event::Address;
use seismograph_rallocator::callers::AddressLookup;

use super::super::data::{AllocationStackFilter, ThreadOperationKind, hotspot_stack, primitive_stack};
use super::super::filter::symbol_path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Frames {
    pub(super) application: Vec<String>,
    pub(super) complete: Vec<String>,
    pub(super) relative_known: bool,
}

type StackKey<'a> = (&'a [Address], &'a [Address], bool, bool);

pub(super) struct Cache<'a> {
    lookups: HashMap<u64, &'a AddressLookup>,
    frames: HashMap<StackKey<'a>, Arc<Frames>>,
}

impl<'a> Cache<'a> {
    pub(super) fn new(addresses: &'a [AddressLookup]) -> Self {
        Self {
            lookups: addresses.iter().map(|lookup| (lookup.address, lookup)).collect(),
            frames: HashMap::new(),
        }
    }

    pub(super) fn get(&mut self, raw: &'a [Address], boundary: &'a [Address], kind: ThreadOperationKind, attributed: bool) -> Arc<Frames> {
        Arc::clone(
            self.frames
                .entry((raw, boundary, kind.is_allocation(), attributed))
                .or_insert_with(|| {
                    let common = common_suffix(raw, boundary).len();
                    // One shared thread-entry frame is not a useful task root. Never remove
                    // the entire captured stack or guess a root from spawn/type symbols.
                    let root = attributed
                        .then(|| instrumentation_root(raw, &self.lookups))
                        .flatten()
                        .or_else(|| (common >= 2 && common < raw.len()).then_some(raw.len() - common));
                    let relative_known = root.is_some();
                    let application = root.map_or(raw, |root| &raw[..root]);
                    let format = |raw: &[Address], filter| {
                        let addresses = raw.iter().map(|address| address.get()).collect::<Vec<_>>();
                        if kind.is_allocation() {
                            hotspot_stack(&addresses, &self.lookups, filter)
                        } else {
                            primitive_stack(&addresses, &self.lookups, filter)
                        }
                    };
                    Arc::new(Frames {
                        application: format(application, AllocationStackFilter::Application),
                        complete: format(raw, AllocationStackFilter::All),
                        relative_known,
                    })
                }),
        )
    }
}

fn instrumentation_root(raw: &[Address], lookups: &HashMap<u64, &AddressLookup>) -> Option<usize> {
    // Oxidizer's poll hooks intentionally never capture boundary backtraces.
    // Their wrapper frames in the operation stack still identify where the
    // instrumented user-future poll began, without changing the recording path.
    const ROOTS: [&str; 2] = [
        "oxidizer_rt::seismograph::TaskTelemetryFuture::poll",
        "oxidizer_rt::tasks::remote_task_future::RemoteTaskFuture::poll",
    ];
    raw.iter().enumerate().skip(1).find_map(|(index, address)| {
        let symbol = lookups.get(&address.get())?.symbol.as_deref()?;
        let path = symbol_path(symbol)?;
        ROOTS
            .iter()
            .any(|root| {
                path.strip_prefix(root)
                    .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with("::"))
            })
            .then_some(index)
    })
}

pub(super) fn common_suffix<'a>(left: &'a [Address], right: &[Address]) -> &'a [Address] {
    let common = left
        .iter()
        .rev()
        .zip(right.iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    &left[left.len() - common..]
}
