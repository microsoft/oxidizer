// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared fixtures and helpers for this crate's own tests.
//!
//! Anything used by more than one test module lives here, so the production modules carry only
//! production code. Module-local helpers stay where they are used.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytesbuf::mem::{CallbackMemory, GlobalPool};
use bytesbuf::{BytesBuf, BytesView};
use thread_aware::{Thread, ThreadAware};

use crate::core::{Compress, Compression, CompressionInternal, Output};
use crate::{Error, Result};

/// What a [`counting_memory`] provider has been asked to do.
///
/// A cloneable handle onto the counters, so a test can hold one after the provider has been moved
/// into a [`Resources`][crate::Resources].
#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryActivity {
    reservations: Arc<AtomicUsize>,
    relocations: Arc<AtomicUsize>,
}

impl MemoryActivity {
    /// How many times a buffer has been reserved from the provider.
    pub(crate) fn reservations(&self) -> usize {
        self.reservations.load(Ordering::SeqCst)
    }

    /// How many times the provider has been told it moved.
    pub(crate) fn relocations(&self) -> usize {
        self.relocations.load(Ordering::SeqCst)
    }
}

/// The state a [`counting_memory`] provider carries: the wrapped provider and the counters.
///
/// `CallbackMemory`'s reservation function is a bare `fn` pointer and cannot capture, so everything
/// it needs lives here.
#[derive(Clone, Debug)]
pub(crate) struct CountingData {
    inner: GlobalPool,
    activity: MemoryActivity,
}

impl ThreadAware for CountingData {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.activity.relocations.fetch_add(1, Ordering::SeqCst);
        self.inner.relocate(source, destination);
    }
}

fn count_reserve(data: &CountingData, min_bytes: usize) -> BytesBuf {
    data.activity.reservations.fetch_add(1, Ordering::SeqCst);
    data.inner.reserve(min_bytes)
}

/// A memory provider that records what is asked of it.
///
/// For tests that need to prove the crate drew its buffers from the caller's provider rather than
/// one of its own, or that a relocation reached it. Built on `bytesbuf`'s own `CallbackMemory`
/// rather than a hand-written [`Memory`] impl, which is the extension point that exists for this.
pub(crate) fn counting_memory() -> (CallbackMemory<CountingData>, MemoryActivity) {
    let activity = MemoryActivity::default();
    let data = CountingData {
        inner: GlobalPool::new(),
        activity: activity.clone(),
    };

    (CallbackMemory::new(data, count_reserve), activity)
}

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
