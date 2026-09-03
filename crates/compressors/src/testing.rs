// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared fixtures and helpers for this crate's own tests.
//!
//! Anything used by more than one test module lives here, so the production modules carry only
//! production code. Module-local helpers stay where they are used.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;

use crate::core::{Compress, Compression, CompressionInternal, Output};
use crate::{Error, Result};

/// A view over `bytes`, allocated from a throwaway pool.
pub(crate) fn view(bytes: &[u8]) -> BytesView {
    BytesView::copied_from_slice(bytes, &GlobalPool::new())
}

/// A view over `bytes` split into `segment` sized spans, exercising the multi-segment paths.
pub(crate) fn fragmented(bytes: &[u8], segment: usize) -> BytesView {
    let memory = GlobalPool::new();
    BytesView::from_views(bytes.chunks(segment).map(|chunk| BytesView::copied_from_slice(chunk, &memory)))
}

/// A chunk size, for the builders and pumps that require a non-zero one.
///
/// # Panics
///
/// Panics if `size` is zero, which is a mistake in the calling test.
pub(crate) fn chunk(size: usize) -> NonZeroUsize {
    NonZeroUsize::new(size).expect("test chunk sizes are non-zero literals")
}

/// A fixture that only ever reports progress, for exercising callers that must keep polling rather
/// than treat a progress step as output.
#[derive(Debug)]
pub(crate) struct ProgressCompression {
    pulls: Arc<AtomicUsize>,
}

impl ProgressCompression {
    pub(crate) fn new(pulls: Arc<AtomicUsize>) -> Self {
        Self { pulls }
    }
}

impl Compression for ProgressCompression {
    type Mode = Compress;
}

impl CompressionInternal for ProgressCompression {
    fn push(&mut self, _input: BytesView) -> Result<()> {
        Ok(())
    }

    fn end_input(&mut self) {}

    fn pull(&mut self) -> Result<Output> {
        self.pulls.fetch_add(1, Ordering::Relaxed);
        Ok(Output::Progress)
    }

    // No caller on the path this fixture exists for asks for the byte counters; they are here only
    // because the trait requires them.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    fn total_in(&self) -> u64 {
        0
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    fn total_out(&self) -> u64 {
        0
    }
}

/// A fixture that always asks for input and always rejects it, for exercising callers that must
/// propagate a `push` failure rather than the specific reasons a real engine's `push` can fail.
#[derive(Debug)]
pub(crate) struct RejectsPush;

impl Compression for RejectsPush {
    type Mode = Compress;
}

impl CompressionInternal for RejectsPush {
    // Accepting input would make this fixture, whose whole purpose is to reject it, ask for input
    // endlessly instead. The mutant hangs rather than failing, so no verdict is available.
    #[cfg_attr(test, mutants::skip)]
    fn push(&mut self, _input: BytesView) -> Result<()> {
        Err(Error::invalid_state("this fixture always rejects pushed input"))
    }

    fn end_input(&mut self) {}

    fn pull(&mut self) -> Result<Output> {
        Ok(Output::NeedInput)
    }

    // No caller on the path this fixture exists for asks for the byte counters; they are here only
    // because the trait requires them.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    fn total_in(&self) -> u64 {
        0
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    fn total_out(&self) -> u64 {
        0
    }
}
