// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "Shared test fixture construction should fail immediately")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use seismograph_rallocator::snapshot::Snapshot;
use seismograph_rallocator::{encode, encoded_len};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn render_html(snapshot: &Snapshot, test_name: &str) -> String {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!(
        "integration-{test_name}-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();

    let input = directory.join("capture.rallocator");
    let output = directory.join("report.html");
    let mut bytes = vec![0; encoded_len(snapshot).unwrap()];
    encode(snapshot, &mut bytes).unwrap();
    fs::write(&input, bytes).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_seismograph"))
        .args(["snapshot", "html"])
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "snapshot HTML command failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let html = fs::read_to_string(&output).unwrap();
    fs::remove_dir_all(directory).unwrap();
    html
}
