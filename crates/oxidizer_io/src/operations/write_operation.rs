// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Weak};
use std::{mem, thread};

use bytes::Buf;
use negative_impl::negative_impl;
use tracing::{Level, event};

use crate::mem::Sequence;
use crate::pal::ElementaryOperationKey;
use crate::{BeginResult, BoundPrimitiveRef, Operation, Resources, UserResource};

/// An I/O operation that writes some bytes of data.
///
/// User code typically does not need to interact with this type directly, as it is primarily used
/// to build higher-level I/O endpoint implementations.
///
/// Write operations are required to fully consume their input data. If an operation indicates
/// a partial consumption of data, the I/O subsystem considers the operation failed.
#[derive(Debug)]
pub struct WriteOperation<const MAX_CHUNKS: usize> {
    // This ensures that the primitive we are operating on is kept alive until we issue the I/O
    // to the operating system (assuming that it is even alive once we start the operation - there
    // may be a delay between the user enqueuing I/O and the operation actually starting).
    primitive: Weak<BoundPrimitiveRef>,

    offset: u64,
    user_resources: Option<Box<dyn UserResource>>,
    sequence: Sequence,
    resources: Arc<Resources>,
}

impl<const MAX_CHUNKS: usize> WriteOperation<MAX_CHUNKS> {
    /// # Panics
    ///
    /// Panics if the sequence has a length of 0. A write of 0 bytes is more likely to be a
    /// programming error. If you truly need to write 0 bytes, model it as a control operation.
    pub(crate) fn new(
        primitive: Weak<BoundPrimitiveRef>,
        sequence: Sequence,
        resources: Arc<Resources>,
    ) -> Self {
        assert_ne!(sequence.len(), 0);

        debug_assert_ne!(MAX_CHUNKS, 0);

        Self {
            primitive,
            offset: 0,
            user_resources: None,
            sequence,
            resources,
        }
    }

    /// For seekable I/O primitives (e.g. files), sets the offset where the operation
    /// should be performed.
    ///
    /// An I/O endpoint implementation must not set an offset (i.e. not call this method)
    /// if the I/O primitive has no concept of seeking/offset. Even specifying a value of 0
    /// indicates that a concept of "offset" is recognized by the I/O primitive.
    #[must_use]
    pub const fn with_offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    /// Attaches an arbitrary value to the operation, to be dropped once the operation completes.
    /// This can be used to ensure some resources remain alive while the operation is in progress,
    /// even if the future driving the operation is dropped.
    #[must_use]
    pub fn with_resources(mut self, resources: impl UserResource) -> Self {
        self.user_resources = Some(Box::new(resources));
        self
    }

    /// Begins an asynchronous write operation by calling the provided callback for each elementary
    /// I/O operation that must be started to complete the operation.
    ///
    /// The operation succeeds when all the data in the provided `Sequence` has been written.
    ///
    /// The callback must perform exactly one system call per invocation and return a `BeginResult`
    /// indicating whether the operation completed synchronously or was scheduled for asynchronous
    /// completion.
    ///
    /// The callback may be called at any point in the future - it is not guaranteed to be called
    /// during the call to this method. If the primitive is closed before the callback is started,
    /// the operation is canceled.
    ///
    /// If the operation succeeds, the output will be the same `SequenceBuilder` that
    /// was used to create the operation, now with some additional read bytes appended to it.
    ///
    /// # Number of callback invocations
    ///
    /// The callback is invoked once for each elementary I/O operation that needs to be executed.
    /// This amount is determined by the memory layout of the `Sequence` used to provide the data
    /// to be written, batching up to `MAX_CHUNKS` contiguous chunks of memory into a single
    /// operation. This batching is also called vectored I/O (as in "a vector of buffers").
    ///
    /// For optimal performance and efficiency, batch as many chunks as possible into the same
    /// operation. The I/O memory management logic does not make any guarantees about the
    /// memory layout of byte sequences, so any specific numbers are unknown at compile time.
    /// Additional constraints may be imposed by the underlying native I/O APIs, some of which
    /// may only support operating on one contiguous memory buffer at a time or impose special
    /// requirements for batching, such as using page-aligned memory.
    ///
    /// The callback invocations create a sequence of low-level I/O operations - the first
    /// such operation must complete before the next one can be started. The I/O operation as a
    /// whole is considered completed when all the low-level operations started by the
    /// callbacks have completed, only returning a success status if every native I/O operation
    /// succeeded. The target offset used with each invocation is automatically incremented by the
    /// number of bytes already written.
    ///
    /// # Panics
    ///
    /// Panics if the operation offset overflows u64 bounds when incremented during the operation.
    ///
    /// Panics if the callback was called but it never requested the platform-specific system call
    /// parameters from the `WriteOperationArgs` struct, indicating that it did not perform any
    /// system call as was required.
    pub async fn begin<C>(mut self, callback: C) -> Result<(), crate::Error>
    where
        C: for<'operation, 'callback> Fn(
                &'callback BoundPrimitiveRef,
                WriteOperationArgs<'operation, 'callback, MAX_CHUNKS>,
            ) -> BeginResult<()>
            + Send
            + 'static,
    {
        // Note: in the future, all of this logic here may be deferred and batched. The current
        // implementation does it all immediately but while still valid, this is a special case.

        let mut total_bytes_written: u64 = 0;

        // We grab up to MAX_CHUNKS spans at a time into one vectored write (with MAX_CHUNKS=1
        // used for non-vectored writes).
        //
        // We execute these elementary operations sequentially as a safe default because not all
        // I/O endpoints support concurrent write operations (filesystem does, but sockets do not,
        // for example). A future version may add something like `.concurrent()` to allow concurrent
        // execution when the conditions are right.
        while !self.sequence.is_empty() {
            // We keep the primitive alive until we start the native operation. Once the native
            // operation has started (i.e. once the callback has been called), we no longer need
            // to keep it alive and drop this to allow cleanup if this logical operation is the
            // last thing keeping the primitive alive, at which point the OS may simply cancel the
            // native operation itself (which is fine).
            let Some(primitive) = self.primitive.upgrade() else {
                // The primitive was already closed before we started the operation.
                return Err(crate::Error::Canceled);
            };

            let offset = self
                .offset
                .checked_add(total_bytes_written)
                .expect("exceeding u64 bounds when seeking write head is not realistic unless a bad offset was specified at the start");

            let memory_guard = self.sequence.extend_lifetime();

            let mut operation = Operation::new(
                offset,
                memory_guard,
                self.user_resources.take(),
                self.resources.pal(),
            );

            event!(
                Level::TRACE,
                message = "elementary begin",
                key = operation.elementary_operation_key().0
            );

            let result_rx = operation.take_result_rx();
            let elementary_operation_key = operation.elementary_operation_key();

            // We mark these as having 'static lifetime because the true lifetime of these buffers
            // is the lifetime of the native I/O operation, which is not expressible in Rust.
            // The only thing we are allowed to do here is to pass this data set to `args` of
            // the callback, which is thereafter allowed to pass these to the OS since by
            // definition the native I/O operation ends when the OS is done with this data.
            let mut chunks: [&'static [u8]; MAX_CHUNKS] = [&[]; MAX_CHUNKS];

            let mut chunks_used = {
                // SAFETY: See comments on `chunks` above.
                let static_sequence: &'static Sequence =
                    unsafe { mem::transmute::<&Sequence, &'static Sequence>(&self.sequence) };

                static_sequence.chunks_as_slices_vectored(&mut chunks)
            };

            // This guarantees that we fit in u32::MAX per elementary operation.
            // This is a requirement of the underlying platform API, which we mirror.
            // If the write operation wants to write more bytes, we just split the write
            // to multiple elementary operations (same as we do with vectored limits).
            constrain_chunks(&mut chunks, &mut chunks_used);

            let mut bytes_written_synchronously: u32 = 0;

            // sum() is safe due to crate-level requirement of 64-bit usize
            let expected_bytes_written = u32::try_from(
                chunks
                    .iter()
                    .take(chunks_used)
                    .map(|c| c.len())
                    .sum::<usize>(),
            )
            .expect("guarded by constrain_chunks()");

            let expected_bytes_written_usize = usize::try_from(expected_bytes_written)
                .expect("crate-level requirement is at least 64-bit usize, so this is safe");

            let args = WriteOperationArgs {
                chunks,
                chunk_count: chunks_used,
                elementary_operation_key: Some(elementary_operation_key),
                bytes_written: &mut bytes_written_synchronously,
            };

            let synchronous_result = callback(&primitive, args);

            // The operation has started (or synchronously completed) and we no longer have
            // any need to keep the primitive alive (especially if we are going to suspend and
            // await). If someone cares about the operation completing asynchronously, they
            // need to keep the primitive alive themselves.
            drop(primitive);

            match synchronous_result {
                BeginResult::Asynchronous => {
                    // Everything is going well, now we just need to wire up to await the result.
                    event!(
                        Level::TRACE,
                        message = "elementary async",
                        key = elementary_operation_key.0,
                    );
                }
                BeginResult::CompletedSynchronously(result) => {
                    event!(
                        Level::TRACE,
                        message = "elementary immediate",
                        result = ?result,
                        key = elementary_operation_key.0,
                        bytes_written_synchronously
                    );

                    match result {
                        Ok(()) => {
                            // The operation completed synchronously. This means we will not get a
                            // completion notification and must handle the result inline.

                            verify_bytes_written_expectations(
                                offset,
                                bytes_written_synchronously,
                                expected_bytes_written,
                            )?;

                            // Finished consuming this batch of chunks.
                            // Skip over the data we processed so we can do the next batch.
                            total_bytes_written = total_bytes_written
                                .checked_add(u64::from(expected_bytes_written)).expect(
                                    "we cannot be writing more than u64::MAX bytes in a single write operation",
                                );

                            self.sequence.advance(expected_bytes_written_usize);
                            continue;
                        }
                        Err(e) => {
                            // Something went wrong. In this case, the elementary operation
                            // was not initialized by the OS and no completion notification will
                            // be received by the completion queue.
                            return Err(e);
                        }
                    }
                }
            }

            // We register the operation in the resource set so that the I/O driver
            // can find it once we receive the completion notification.
            self.resources
                .operations_mut()
                .insert(elementary_operation_key, operation);

            let bytes_written = result_rx
                .await
                .expect("I/O driver side of an I/O operation vanished while awaiting result")?;

            verify_bytes_written_expectations(offset, bytes_written, expected_bytes_written)?;

            // Finished consuming this batch of chunks.
            // Skip over the data we processed so we can do the next batch.
            total_bytes_written = total_bytes_written
                .checked_add(u64::from(expected_bytes_written))
                .expect("overflowing u64 when advancing written bytes cursor is unrealistic");
            self.sequence.advance(expected_bytes_written_usize);
        }

        Ok(())
    }
}

/// Constraints the chunks used as part of a vectored operation to address no more than
/// `u32::MAX` bytes of data because that is an underlying platform API limitation.
fn constrain_chunks(chunks: &mut [&'static [u8]], chunks_used: &mut usize) {
    let mut remaining = usize::try_from(u32::MAX).expect("crate requires at least 64-bit usize");

    for (chunk_index, chunk) in chunks.iter_mut().enumerate().take(*chunks_used) {
        let desired_chunk_len = chunk.len().min(remaining);

        if desired_chunk_len != chunk.len() {
            // Restrict size of this chunk and finish.
            if desired_chunk_len == 0 {
                *chunks_used = chunk_index;
                return;
            }

            *chunk = chunk
                .get(..desired_chunk_len)
                .expect("guarded by min() above");
            *chunks_used = chunk_index
                .checked_add(1)
                .expect("we can never exceed original value, only reduce it");
        }

        remaining = remaining
            .checked_sub(chunk.len())
            .expect("guarded by min() above");
    }
}

fn verify_bytes_written_expectations(
    offset: u64,
    bytes_written_actual: u32,
    bytes_written_expected: u32,
) -> Result<(), crate::Error> {
    // The operation API contract requires that all bytes be written.
    if bytes_written_actual == bytes_written_expected {
        return Ok(());
    }

    // We assert that we written less than required (which is a logic error).
    // If we written more than required, we have a memory safety violation and it is
    // not safe to continue process execution because we may be in a state where we
    // are exposing memory we are not allowed to expose.
    assert!(bytes_written_actual < bytes_written_expected);

    Err(crate::Error::ContractViolation(format!(
        "elementary operation at offset {offset} was required to write all bytes but wrote fewer bytes: {bytes_written_actual} != {bytes_written_expected}"
    )))
}

/// Arguments provided by the I/O subsystem to the `begin()` callback that executes one elementary
/// I/O operation to induce the operating system to write some bytes of data.
///
/// # Resource management
///
/// References with the `'operation` lifetime are valid for the entire duration of the
/// elementary operation, up until the operating system notifies us that the operation has been
/// completed.
///
/// This means the references remain valid even when moved out of the domain of the Rust borrow
/// checker as pointers, and may be handed to the operating system. The full I/O operation
/// lifetime does not map to a single Rust lifetime - the `'operation` lifetime is merely a marker.
///
/// Note that `'operation` refers to the elementary native I/O operation, not the `WriteOperation`.
/// One logical operation may involve multiple elementary native I/O operations, one per batch of
/// chunks containing data to be written.
///
/// References with the `'callback` lifetime are valid for the duration of the callback
/// that provides this arguments object.
#[derive(Debug)]
pub struct WriteOperationArgs<'operation, 'callback, const MAX_CHUNKS: usize> {
    // Some of these items may be unused (`&[]`) if we did not need MAX_CHUNKS.
    chunks: [&'operation [u8]; MAX_CHUNKS],

    // How many of the above slots are actually used.
    chunk_count: usize,

    bytes_written: &'callback mut u32,

    // This is Option because it can only be called once (to consume it) because each `begin()` is
    // only intended to start a single system call. We use the Option to protect against double-get
    // and to ensure (via Drop check) that the caller does consume it (if not - no syscall made?!)
    //
    // Of course, even if the callback asks for the system call parameters exactly once, there is
    // no guarantee we can get here that it will use the parameters to actually make a system call
    // but if it does not, that is merely a memory leak and not a safety issue, so good enough.
    elementary_operation_key: Option<ElementaryOperationKey>,
}

impl<'operation, const MAX_CHUNKS: usize> WriteOperationArgs<'operation, '_, MAX_CHUNKS> {
    /// Iterates over the chunks of data to be written by the elementary I/O operation. Write
    /// operations are required to fully consume all data in the chunk - partial consumes are
    /// considered errors.
    ///
    /// The I/O subsystem guarantees that this memory is pinned and will
    /// not move during the I/O operation.
    pub fn chunks(&self) -> impl Iterator<Item = &'operation [u8]> {
        self.chunks.iter().take(self.chunk_count).copied()
    }

    /// Gets an exclusive reference to a field that must be set to the number of bytes that were
    /// written by the operation if the operation completes synchronously, as signaled by
    /// a callback return value of [`BeginResult::CompletedSynchronously`].
    ///
    /// This field is ignored if the operation will complete asynchronously, as signaled by a
    /// callback return value of [`BeginResult::Asynchronous`].
    pub const fn bytes_written_synchronously_as_mut(&mut self) -> &mut u32 {
        self.bytes_written
    }

    /// # Panics
    ///
    /// Panics if called more than once.
    pub(crate) const fn consume_elementary_operation_key(&mut self) -> ElementaryOperationKey {
        self.elementary_operation_key
            .take()
            .expect("elementary operation consumed more than once from WriteOperationArgs")
    }
}

impl<const MAX_CHUNKS: usize> Drop for WriteOperationArgs<'_, '_, MAX_CHUNKS> {
    fn drop(&mut self) {
        if thread::panicking() {
            // No point to double-panic, that will just obscure the original panic.
            return;
        }

        assert!(
            self.elementary_operation_key.is_none(),
            "system call parameters not consumed from WriteOperationArgs - no system call could have been made"
        );
    }
}

#[negative_impl]
impl<const MAX_CHUNKS: usize> !Send for WriteOperationArgs<'_, '_, MAX_CHUNKS> {}
#[negative_impl]
impl<const MAX_CHUNKS: usize> !Sync for WriteOperationArgs<'_, '_, MAX_CHUNKS> {}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::integer_division,
        clippy::cast_possible_truncation,
        reason = "this is just test code, anomalies are nothing to fear"
    )]

    use std::collections::VecDeque;
    use std::io::ErrorKind;
    use std::pin::pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll};

    use bytes::BufMut;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::pal::{MockPlatform, PlatformFacade};
    use crate::testing::{
        assert_panic, bind_dummy_primitive, expect_elementary_operations,
        new_failed_completion_notification, new_successful_completion_notification,
        use_default_memory_pool, use_simulated_completion_queue,
        with_partial_io_test_harness_and_platform,
    };
    use crate::{ERR_POISONED_LOCK, ReserveOptions};

    #[test]
    fn thread_mobile_type() {
        assert_impl_all!(WriteOperation<1>: Send);
    }

    #[test]
    fn args_is_always_single_threaded_type() {
        assert_not_impl_any!(WriteOperationArgs<1>: Send, Sync);
    }

    #[test]
    fn write_empty() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        // Nothing to actually write, so 0 elementary operations get executed.
        expect_elementary_operations(0, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let payload = Sequence::default();

            assert_panic!(primitive.write_bytes::<1>(payload));
        });
    }

    fn validate_successful_write<const MAX_CHUNKS: usize>(block_count: usize, is_async: bool) {
        // The block size does not fundamentally matter, we just pick something non-round to
        // reduce the probability of happy coincidences hiding defects.
        const BLOCK_SIZE: usize = 99;

        let mut platform = MockPlatform::new();

        use_default_memory_pool::<BLOCK_SIZE>(&mut platform);

        let total_bytes = block_count * BLOCK_SIZE;
        let expected_elementary_operation_count = block_count.div_ceil(MAX_CHUNKS);
        expect_elementary_operations(expected_elementary_operation_count, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness
                .context
                .reserve(total_bytes, ReserveOptions::default());
            payload_builder.put_bytes(0, total_bytes);

            let payload = payload_builder.consume_all();

            let remaining_chunk_lengths =
                Arc::new(Mutex::new(VecDeque::from(vec![BLOCK_SIZE; block_count])));

            let write_future = primitive.write_bytes::<MAX_CHUNKS>(payload).begin({
                let remaining_chunk_lengths = Arc::clone(&remaining_chunk_lengths);

                move |_primitive, mut args| {
                    assert!(remaining_chunk_lengths.lock().unwrap().len() >= args.chunk_count);

                    let expected_chunk_lengths = remaining_chunk_lengths
                        .lock()
                        .unwrap()
                        .drain(..args.chunk_count)
                        .collect::<Vec<_>>();

                    for (expected_len, actual) in expected_chunk_lengths.iter().zip(args.chunks()) {
                        assert_eq!(expected_len, &actual.len());
                    }

                    let bytes_written = expected_chunk_lengths.iter().sum::<usize>() as u32;

                    if is_async {
                        queue_simulation.completed.lock().unwrap().push_back(
                            new_successful_completion_notification(
                                args.consume_elementary_operation_key(),
                                bytes_written,
                            ),
                        );

                        BeginResult::Asynchronous
                    } else {
                        // We must consume the elementary operation key to indicate that we have made
                        // (or simulated) a system call, because this is mandatory in real code.
                        args.consume_elementary_operation_key();

                        *args.bytes_written_synchronously_as_mut() = bytes_written;
                        BeginResult::CompletedSynchronously(Ok(()))
                    }
                }
            });

            let mut write_future = pin!(write_future);

            loop {
                let mut cx = Context::from_waker(futures::task::noop_waker_ref());
                match write_future.as_mut().poll(&mut cx) {
                    Poll::Ready(Ok(())) => break,
                    Poll::Ready(r) => r.unwrap(),
                    Poll::Pending => {}
                }

                test_harness.driver.borrow_mut().process_completions(0);
            }

            // We verify that we saw all expected chunks of bytes in the callback.
            assert_eq!(0, remaining_chunk_lengths.lock().unwrap().len());
        });
    }

    #[test]
    fn write_single_block_non_vectored_async() {
        validate_successful_write::<1>(1, true);
    }

    #[test]
    fn write_single_block_vectored_async() {
        validate_successful_write::<8>(1, true);
    }

    #[test]
    fn write_multi_block_non_vectored_async() {
        validate_successful_write::<1>(10, true);
    }

    #[test]
    fn write_multi_block_vectored_async() {
        validate_successful_write::<4>(10, true);
    }

    #[test]
    fn write_single_block_non_vectored_sync() {
        validate_successful_write::<1>(1, false);
    }

    #[test]
    fn write_single_block_vectored_sync() {
        validate_successful_write::<8>(1, false);
    }

    #[test]
    fn write_multi_block_non_vectored_sync() {
        validate_successful_write::<1>(10, false);
    }

    #[test]
    fn write_multi_block_vectored_sync() {
        validate_successful_write::<4>(10, false);
    }

    #[test]
    #[cfg(not(miri))] // Miri gets real slow with large/many memory allocations, so skip here.
    fn write_is_split_on_u32_max() {
        // When someone requests to write a giant amount of data, we split it into individual
        // writes no bigger than u32::MAX each. We try to write 5 GB here, so we expect to see
        // Elementary operation 1: 4 GB - 1 bytes
        // Elementary operation 2: 1 GB + 1 bytes
        // Note that u32::MAX is 4 GB - 1.

        use crate::mem::SequenceBuilder;

        // To avoid a giant memory allocation, we replicate a single block of 10 MB over and over.
        // This does require a large vectored write, but that is not a problem - our vectoring
        // limit is not meaningfully constrained (except by stack size).
        const BLOCK_SIZE: usize = 10 * 1024 * 1024;

        const TOTAL_BYTES: usize = 5 * 1024 * 1024 * 1024;
        const EXPECTED_FIRST_WRITE_LEN: usize = u32::MAX as usize;
        const EXPECTED_SECOND_WRITE_LEN: usize = TOTAL_BYTES - EXPECTED_FIRST_WRITE_LEN;
        const TOTAL_BLOCKS: usize = TOTAL_BYTES / BLOCK_SIZE;

        let mut platform = MockPlatform::new();

        use_default_memory_pool::<BLOCK_SIZE>(&mut platform);
        expect_elementary_operations(2, &mut platform);
        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut ten_megabytes = test_harness
                .context
                .reserve(BLOCK_SIZE, ReserveOptions::default());

            ten_megabytes.put_bytes(0, BLOCK_SIZE);
            let ten_megabytes = ten_megabytes.consume_all();

            let mut payload = SequenceBuilder::new();

            for _ in 0..(TOTAL_BYTES / BLOCK_SIZE) {
                payload.append(ten_megabytes.clone());
            }

            assert_eq!(payload.len(), TOTAL_BYTES);

            let remaining_write_lengths = Arc::new(Mutex::new(VecDeque::from(vec![
                EXPECTED_FIRST_WRITE_LEN,
                EXPECTED_SECOND_WRITE_LEN,
            ])));

            // We use TOTAL_BLOCKS here to avoid the vectoring limit from being the reason
            // that the write is split - vectoring logic would allow accepting all data at once.
            let write_future = primitive
                .write_bytes::<TOTAL_BLOCKS>(payload.consume_all())
                .begin({
                    let remaining_write_lengths = Arc::clone(&remaining_write_lengths);

                    move |_primitive, mut args| {
                        let expected_write_length =
                            remaining_write_lengths.lock().unwrap().pop_front();
                        let write_length = args.chunks().map(<[u8]>::len).sum::<usize>();

                        assert_eq!(expected_write_length, Some(write_length));

                        // We must consume the elementary operation key to indicate that we have made
                        // (or simulated) a system call, because this is mandatory in real code.
                        args.consume_elementary_operation_key();

                        *args.bytes_written_synchronously_as_mut() = write_length as u32;
                        BeginResult::CompletedSynchronously(Ok(()))
                    }
                });

            let mut write_future = pin!(write_future);

            loop {
                let mut cx = Context::from_waker(futures::task::noop_waker_ref());
                match write_future.as_mut().poll(&mut cx) {
                    Poll::Ready(Ok(())) => break,
                    Poll::Ready(r) => r.unwrap(),
                    Poll::Pending => {}
                }

                test_harness.driver.borrow_mut().process_completions(0);
            }

            // We verify that we saw all expected writes in the callback.
            assert_eq!(0, remaining_write_lengths.lock().unwrap().len());
        });
    }

    #[test]
    fn write_failed_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future =
                primitive
                    .write_bytes::<1>(payload)
                    .begin(move |_primitive, mut args| {
                        queue_simulation.completed.lock().unwrap().push_back(
                            new_failed_completion_notification(
                                args.consume_elementary_operation_key(),
                            ),
                        );

                        BeginResult::Asynchronous
                    });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Err(_))));
        });
    }

    #[test]
    fn write_failed_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future =
                primitive
                    .write_bytes::<1>(payload)
                    .begin(move |_primitive, mut args| {
                        // We must consume the elementary operation key to indicate that we have made
                        // (or simulated) a system call, because this is mandatory in real code.
                        args.consume_elementary_operation_key();

                        BeginResult::CompletedSynchronously(Err(std::io::Error::new(
                            ErrorKind::AlreadyExists,
                            "hey what did you do",
                        )
                        .into()))
                    });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Err(_))));
        });
    }

    #[test]
    fn written_too_many_bytes_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future =
                primitive
                    .write_bytes::<1>(payload)
                    .begin(move |_primitive, mut args| {
                        assert_eq!(args.chunk_count, 1);
                        assert_eq!(args.chunks().next().unwrap().len(), 100);

                        // We have 100 bytes of data but somehow write 101! That's not allowed.
                        queue_simulation.completed.lock().unwrap().push_back(
                            new_successful_completion_notification(
                                args.consume_elementary_operation_key(),
                                101,
                            ),
                        );

                        BeginResult::Asynchronous
                    });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());

            // We expect a panic from here due to memory safety violation.
            assert_panic!(_ = write_future.as_mut().poll(&mut cx));
        });
    }

    #[test]
    fn written_too_many_bytes_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future =
                primitive
                    .write_bytes::<1>(payload)
                    .begin(move |_primitive, mut args| {
                        assert_eq!(args.chunk_count, 1);
                        assert_eq!(args.chunks().next().unwrap().len(), 100);

                        // We must consume the elementary operation key to indicate that we have made
                        // (or simulated) a system call, because this is mandatory in real code.
                        args.consume_elementary_operation_key();

                        *args.bytes_written_synchronously_as_mut() = 101;
                        BeginResult::CompletedSynchronously(Ok(()))
                    });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());

            // We expect a panic from here due to memory safety violation.
            assert_panic!(_ = write_future.as_mut().poll(&mut cx));
        });
    }

    #[test]
    fn written_not_enough_bytes_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);
        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future =
                primitive
                    .write_bytes::<1>(payload)
                    .begin(move |_primitive, mut args| {
                        assert_eq!(args.chunk_count, 1);
                        assert_eq!(args.chunks().next().unwrap().len(), 100);

                        // We have 100 bytes of data but somehow write 99! That's not allowed.
                        queue_simulation.completed.lock().unwrap().push_back(
                            new_successful_completion_notification(
                                args.consume_elementary_operation_key(),
                                99,
                            ),
                        );

                        BeginResult::Asynchronous
                    });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            // We do not have an API contract for what error it must be.
            assert!(matches!(poll_result, Poll::Ready(Err(_))));
        });
    }

    #[test]
    fn written_not_enough_bytes_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future =
                primitive
                    .write_bytes::<1>(payload)
                    .begin(move |_primitive, mut args| {
                        assert_eq!(args.chunk_count, 1);
                        assert_eq!(args.chunks().next().unwrap().len(), 100);

                        // We must consume the elementary operation key to indicate that we have made
                        // (or simulated) a system call, because this is mandatory in real code.
                        args.consume_elementary_operation_key();

                        *args.bytes_written_synchronously_as_mut() = 99;
                        BeginResult::CompletedSynchronously(Ok(()))
                    });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            // We do not have an API contract for what error it must be.
            assert!(matches!(poll_result, Poll::Ready(Err(_))));
        });
    }

    #[test]
    fn write_offset_reaches_platform() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        let operation_offsets = expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future = primitive
                .write_bytes::<1>(payload)
                .with_offset(21546)
                .begin(move |_primitive, mut args| {
                    let elementary_operation_key = args.consume_elementary_operation_key();

                    // The contract of expect_elementary_operations() says the key is the index
                    // into the operation_offsets list.
                    assert_eq!(
                        operation_offsets.lock().expect(ERR_POISONED_LOCK)
                            [elementary_operation_key.0],
                        Some(21546)
                    );

                    queue_simulation.completed.lock().unwrap().push_back(
                        new_successful_completion_notification(elementary_operation_key, 100),
                    );

                    BeginResult::Asynchronous
                });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = write_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(()))));
        });
    }

    #[test]
    fn resources_kept_alive_after_future_dropped() {
        let resource = Arc::new(42);
        let weak = Arc::downgrade(&resource);

        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            {
                let primitive = bind_dummy_primitive(&test_harness.context);

                let mut payload_builder =
                    test_harness.context.reserve(100, ReserveOptions::default());
                payload_builder.put_bytes(0, 100);
                let payload = payload_builder.consume_all();

                let write_future = primitive
                    .write_bytes::<1>(payload)
                    .with_resources(resource)
                    .begin(move |_primitive, mut args| {
                        queue_simulation.completed.lock().unwrap().push_back(
                            new_successful_completion_notification(
                                args.consume_elementary_operation_key(),
                                100,
                            ),
                        );

                        BeginResult::Asynchronous
                    });
                let mut write_future = pin!(write_future);

                let mut cx = Context::from_waker(futures::task::noop_waker_ref());
                let poll_result = write_future.as_mut().poll(&mut cx);
                assert!(matches!(poll_result, Poll::Pending));
            }

            // The future has been dropped! However, we expect that our resource is still alive
            // because the operation is still asynchronously ongoing.
            assert!(weak.upgrade().is_some());

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            // Now the operation has completed and the attached resources should be released.
            assert!(weak.upgrade().is_none());
        });
    }

    // We do not do proper cleanup of the test harness (as that may be impossible after a
    // panic), which makes Miri all bothered about resource leaks. So let's not run under Miri.
    #[cfg(not(miri))]
    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn write_no_syscall_panic() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, |test_harness| async move {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future = primitive
                .write_bytes::<1>(payload)
                // We must consume the elementary operation key, even if we don't use it, to
                // indicate that we have made (or simulated) a system call. We do not!
                .begin(move |_primitive, mut args| {
                    *args.bytes_written_synchronously_as_mut() = 100;
                    BeginResult::CompletedSynchronously(Ok(()))
                });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            // Should panic here.
            _ = write_future.as_mut().poll(&mut cx);
        });
    }

    // We do not do proper cleanup of the test harness (as that may be impossible after a
    // panic), which makes Miri all bothered about resource leaks. So let's not run under Miri.
    #[cfg(not(miri))]
    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn two_syscalls_panic() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, |test_harness| async move {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let mut payload_builder = test_harness.context.reserve(100, ReserveOptions::default());
            payload_builder.put_bytes(0, 100);
            let payload = payload_builder.consume_all();

            let write_future =
                primitive
                    .write_bytes::<1>(payload)
                    .begin(move |_primitive, mut args| {
                        // Calling this twice is invalid, as it implies we are making two syscalls
                        // in a single callback, which is not allowed. The second call should panic.
                        args.consume_elementary_operation_key();
                        args.consume_elementary_operation_key();

                        *args.bytes_written_synchronously_as_mut() = 100;
                        BeginResult::CompletedSynchronously(Ok(()))
                    });
            let mut write_future = pin!(write_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            // Should panic here.
            _ = write_future.as_mut().poll(&mut cx);
        });
    }

    static ONE_MEGABYTE: &[u8] = &[0_u8; 1024 * 1024];

    #[test]
    fn constrain_chunks_noop_if_below_u32_max() {
        // 8 MB is well under the limit, nothing should happen.
        let mut chunks = [ONE_MEGABYTE; 8];
        let mut chunks_used = 8;

        constrain_chunks(&mut chunks, &mut chunks_used);
        assert_eq!(chunks_used, 8);

        for chunk in &chunks {
            assert_eq!(chunk.len(), ONE_MEGABYTE.len());
        }
    }

    #[expect(
        clippy::large_stack_arrays,
        reason = "fine in test logic, size is okay-ish still"
    )]
    #[test]
    fn constrain_chunks_constrains_if_above_u32_max() {
        // We exceed u32::MAX by 1 byte + 1 MB.
        // (The 1 byte is because u32::MAX is 4 GB - 1 byte).
        // Expectation: chunks_used -=1 and chunks.last().len() -= 1
        let mut chunks = [ONE_MEGABYTE; 1024 * 4 + 1];
        let mut chunks_used = chunks.len();

        let expected_chunks_used = chunks_used - 1;

        constrain_chunks(&mut chunks, &mut chunks_used);
        assert_eq!(chunks_used, expected_chunks_used);

        for chunk in chunks.iter().take(chunks_used - 1) {
            // Size of any chunk except the last one should not have been modified.
            assert_eq!(chunk.len(), ONE_MEGABYTE.len());
        }

        // Size of last chunk should have been decreased by 1.
        assert_eq!(chunks[chunks_used - 1].len(), ONE_MEGABYTE.len() - 1);
    }
}