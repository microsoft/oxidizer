// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawner diagnostics and explicit runtime selection without an ambient runtime.

#![cfg(feature = "tokio")]

use anyspawn::Spawner;
use thread_aware::{Thread, ThreadAware};

struct RelocationSeen(bool);

impl ThreadAware for RelocationSeen {
    fn relocate(&mut self, _: Option<&Thread>, _: &Thread) {
        self.0 = true;
    }
}

#[test]
fn gamma_round2_default_tokio_spawner_debug_keeps_the_outer_type_name() {
    assert_eq!(format!("{:?}", Spawner::new_tokio(None)), "Spawner(\"tokio\")");
}

#[test]
fn gamma_survivor_explicit_tokio_handle_spawns_outside_entered_runtime_and_keeps_name() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let builder = Spawner::builder_tokio(runtime.handle().clone());
    assert_eq!(format!("{builder:?}"), "SpawnerBuilder { spawner: Spawner(\"tokio(handle)\") }");
    let spawner = builder.build();
    let task = spawner.spawn(async { 42 });
    assert_eq!(runtime.block_on(task).unwrap(), 42);
}

#[test]
fn gamma_survivor_tokio_spawn_anywhere_preserves_payload_without_relocation() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let spawner = Spawner::new_tokio(runtime.handle().clone());
    assert!(
        !runtime
            .block_on(spawner.spawn_anywhere(RelocationSeen(false), |data| async move { data.0 }))
            .unwrap()
    );
    assert_eq!(runtime.block_on(spawner.spawn_blocking(|| 11)).unwrap(), 11);
}
