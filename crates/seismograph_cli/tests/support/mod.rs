// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "Shared test fixture construction should fail immediately")]

use seismograph_rallocator::native::Snapshot;
use seismograph_rallocator::{encode, encoded_len};

pub(crate) fn render_html(snapshot: &Snapshot, test_name: &str) -> String {
    let mut bytes = vec![0; encoded_len(snapshot).unwrap()];
    encode(snapshot, &mut bytes).unwrap();
    render_capture(&bytes, test_name)
}

pub(crate) fn render_capture(bytes: &[u8], _test_name: &str) -> String {
    seismograph_cli::test_util::render_capture(bytes).unwrap()
}
