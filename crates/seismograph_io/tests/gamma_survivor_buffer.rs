// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory buffer metadata regressions.

use seismograph_io::{BufferId, BufferState, IoResourceKind, Resource};

#[test]
fn gamma_survivor_buffer_metadata_preserves_values_and_saturates_spans() {
    let id = BufferId::allocate();
    let state = BufferState::new(Some(id), 0x10203, 0x405);
    assert_eq!(
        (state.id(), state.len(), state.span_count(), state.is_empty()),
        (Some(id), 0x10203, 0x405, false)
    );
    let large = BufferState::new(None, usize::MAX, usize::MAX);
    assert_eq!(large.len(), usize::MAX as u64);
    assert_eq!(large.span_count(), u32::MAX);
    assert_eq!(BufferState::new(None, 0, 0), BufferState::none());
    assert!(BufferState::none().is_empty());
    assert_eq!(Resource::new(IoResourceKind::NamedPipe).kind(), IoResourceKind::NamedPipe);
}
