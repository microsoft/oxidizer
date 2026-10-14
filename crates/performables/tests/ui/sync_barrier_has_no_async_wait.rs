// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::sync::{Sync, barrier::Barrier};

fn main() {
    let barrier = Barrier::<Sync>::new(1);
    drop(barrier.wait_async());
}
