// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::BTreeSet;
use std::sync::Mutex;

/// Each name stays in memory until the process ends, so only names from the setup of a layer may reach this set,
/// never data from a request.
static INTERNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

pub(super) fn intern(name: &str) -> &'static str {
    let mut interned = INTERNED
        .lock()
        .expect("only a lookup, a copy and an insert run under this lock, and none of them panics");
    if let Some(name) = interned.get(name) {
        return name;
    }
    let name: &'static str = String::from(name).leak();
    interned.insert(name);
    name
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::*;

    #[test]
    fn intern_reuses_storage_for_equal_names() {
        let first = intern("http.request.header.x-intern-test");
        let second = intern(&String::from("http.request.header.x-intern-test"));

        assert!(ptr::eq(first, second));
    }
}
