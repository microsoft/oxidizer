// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(not(test))]
use std::mem;
use std::mem::MaybeUninit;
use std::slice;
use std::sync::Arc;

use derive_more::Display;
#[cfg(not(test))]
use static_assertions::{assert_eq_align, assert_eq_size};
use tracing::{Level, event};
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows::Win32::System::IO::OVERLAPPED_ENTRY;
use windows::Win32::System::WindowsProgramming::{
    FILE_SKIP_COMPLETION_PORT_ON_SUCCESS, FILE_SKIP_SET_EVENT_ON_HANDLE,
};
use windows::core::HRESULT;

use crate::pal::windows::{Bindings, CompletionNotificationImpl, PrimitiveImpl};
use crate::pal::{
    BindingsFacade, CompletionNotification, CompletionNotificationFacade, CompletionQueue,
    CompletionQueueWakerFacade, CompletionQueueWakerImpl, Primitive, PrimitiveFacade,
};

/// Max number of I/O operations to dequeue in one go. Presumably getting more data from the OS with
/// a single call is desirable but the exact impact of different values on performance is not known.
///
/// Known aspects of performance impact:
/// * `GetQueuedCompletionStatusEx` duration seems linearly affected under non-concurrent synthetic
///   message load (e.g. 40 us for 1024 items).
const POLL_SIZE_ENTRIES: u32 = 1024;

/// Implements the completion queue concept using a Windows I/O completion port.
#[derive(derive_more::Debug)]
#[debug("{completion_port:?}")]
pub struct CompletionQueueImpl {
    completion_port: Arc<CompletionPort>,

    // This is only used during notification processing but is part of the struct just because
    // we want to allocate it on the heap to avoid placing a giant pile of data on the stack.
    completions: Box<[MaybeUninit<CompletionNotificationImpl>]>,

    bindings: BindingsFacade,
}

impl CompletionQueueImpl {
    pub(super) fn new(bindings: BindingsFacade) -> Self {
        let completion_port = bindings.create_io_completion_port(
            INVALID_HANDLE_VALUE, // We are not binding an existing handle right now.
            None, // Create a new completion port.
            0, // Ignored as we are not binding an existing handle to the port.
            1, // The port is only to be read from by one thread (the current thread).
            ).expect("creating an I/O completion port should never fail unless the OS is critically out of resources");

        let completion_port = Arc::new(CompletionPort::new(PrimitiveImpl::from_handle(
            completion_port,
            bindings.clone(),
        )));

        event!(Level::TRACE, message = "new completion queue", completion_port = %*completion_port, poll_size = POLL_SIZE_ENTRIES);

        Self {
            completion_port,
            // We allocate this entire array on the heap to avoid consuming a large chunk of stack.
            completions: vec![MaybeUninit::uninit(); POLL_SIZE_ENTRIES as usize].into_boxed_slice(),
            bindings,
        }
    }

    /// Asserts that the given number of completions have been initialized
    /// and returns them as a slice.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the given number
    /// of completion notifications have actually been initialized.
    unsafe fn take_completions(&mut self, count: u32) -> &[CompletionNotificationImpl] {
        // Removing MaybeUninit because the caller is making a promise we can do that.
        let as_ptr = self
            .completions
            .as_ptr()
            .cast::<CompletionNotificationImpl>();

        // SAFETY: Forwarding the safety requirements of the caller.
        unsafe { slice::from_raw_parts(as_ptr, count as usize) }
    }

    /// # Safety
    ///
    /// The caller is responsible for ensuring that the given number
    /// of completion notifications have actually been initialized.
    unsafe fn uninitialize_completions(&mut self, count: usize) {
        assert!(count <= self.completions.len());

        // Strictly speaking, this is not necessary because there is no `Drop` implementation
        // in the completion notification type. However, let's be proper - maybe there will be
        // one day and perhaps it simplifies analysis of the code. Revisit when optimizing.
        for index in 0..count {
            // SAFETY: We know it is initialized because the caller says so.
            unsafe {
                self.completions
                    .get_mut(index)
                    .expect("We verified we are in bounds above")
                    .assume_init_drop();
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Mutates | into ^ which is a no-op and false positive.
    fn configure_handle(&self, primitive_handle: HANDLE) -> crate::Result<()> {
        // Why FILE_SKIP_SET_EVENT_ON_HANDLE:
        // https://devblogs.microsoft.com/oldnewthing/20200221-00/?p=103466/
        //
        // SAFETY:
        // * After this call we cannot rely on file handles being secretly treated as events. That
        //   is fine because the whole point is that we do not want to use them as events.
        // * After this call we will not get completion notifications for I/O operations
        //   that complete synchronously. We must handle all synchronous completions inline.
        //   That is also fine and intentional, implemented by the `*Operation` types that only
        //   go into the asynchronous flow if they receive ERROR_IO_PENDING as a result code.
        unsafe {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "constant value guaranteed to fit"
            )]
            self.bindings.set_file_completion_notification_mode(
                primitive_handle,
                (FILE_SKIP_SET_EVENT_ON_HANDLE | FILE_SKIP_COMPLETION_PORT_ON_SUCCESS) as u8,
            )?;
        }

        Ok(())
    }
}

impl CompletionQueue for CompletionQueueImpl {
    fn bind(&self, primitive: &PrimitiveFacade) -> crate::Result<()> {
        let primitive_handle = primitive.as_real().as_handle();

        let completion_port_handle = self.completion_port.as_handle();

        self.bindings.create_io_completion_port(
            *primitive_handle,
            Some(*completion_port_handle),
            0, // Completion key is not used.
            1, // The port is only to be read from by one thread (the current thread).
        )?;

        event!(Level::TRACE, message = "bound primitive", primitive = ?*primitive_handle, completion_port = %*self.completion_port);

        self.configure_handle(*primitive_handle)
    }

    fn process_completions<CB>(&mut self, max_wait_time_millis: u32, mut cb: CB)
    where
        CB: FnMut(&crate::pal::CompletionNotificationFacade),
    {
        let completion_port_handle = self.completion_port.as_handle();

        let as_entries_ptr = self
            .completions
            .as_mut_ptr()
            .cast::<MaybeUninit<OVERLAPPED_ENTRY>>();

        // SAFETY: CompletionNotificationImpl == OVERLAPPED_ENTRY due to repr(transparent)
        // The size is a constant in all cases, so always known to be the right value.
        let entries =
            unsafe { slice::from_raw_parts_mut(as_entries_ptr, POLL_SIZE_ENTRIES as usize) };

        let mut completed_items: u32 = 0;

        let completed_entries = match self.bindings.get_queued_completion_status_ex(
            *completion_port_handle,
            entries,
            &mut completed_items,
            max_wait_time_millis,
            false,
        ) {
            // We got some entries, all is well.
            Ok(()) => {
                // SAFETY: The platform promises it filled this many entries.
                unsafe { self.take_completions(completed_items) }
            }
            // Timeout just means there was nothing to do - no I/O operations completed.
            Err(e) if e.code() == HRESULT::from_win32(WAIT_TIMEOUT.0) => {
                return;
            }
            Err(e) => panic!("unexpected error from GetQueuedCompletionStatusEx: {e:?}"),
        };

        event!(
            Level::TRACE,
            message = "received completion notifications",
            count = completed_items
        );

        for entry in completed_entries {
            if entry.is_wake_up_signal() {
                // We received a wake-up signal. This is a no-op notification that we simply ignore.
                // The goal of this notification is already achieved when we are running this code.
                continue;
            }

            // For anything that is a real completion, we require non-null operation key
            // because we know that this is really a pointer to the OVERLAPPED structure.
            assert_ne!(entry.elementary_operation_key().0, 0);

            event!(
                Level::TRACE,
                message = "operation completed",
                key = entry.elementary_operation_key().0
            );

            process_completion(entry, &mut cb);
        }

        // Drop any completion notifications that we initialized.
        // SAFETY: The platform promises it filled this many entries.
        unsafe {
            self.uninitialize_completions(completed_items as usize);
        }
    }

    fn waker(&self) -> CompletionQueueWakerFacade {
        CompletionQueueWakerImpl::new(Arc::clone(&self.completion_port), self.bindings.clone())
            .into()
    }
}

/// Executes the completion processing callback on a single completion notification.
///
/// In test builds, we need to convert the object into the facade, which is not zero-cost.
///
/// In release builds, we take advantage of the knowledge that the facade is transparent
/// and can be used interchangeably with the real object hiding behind it, for zero-cost usage.
///
/// # Panics
///
/// TODO: Document panics
fn process_completion<CB>(notification: &CompletionNotificationImpl, cb: &mut CB)
where
    CB: FnMut(&CompletionNotificationFacade),
{
    #[cfg(test)]
    {
        let facade = CompletionNotificationFacade::from_real(*notification);
        cb(&facade);
    }
    #[cfg(not(test))]
    #[expect(clippy::transmute_ptr_to_ptr, reason = "TODO: provide rationale")]
    {
        assert_eq_size!(CompletionNotificationImpl, CompletionNotificationFacade);
        assert_eq_align!(CompletionNotificationImpl, CompletionNotificationFacade);

        // SAFETY: The facade is a transparent wrapper in release builds.
        let facade = unsafe {
            mem::transmute::<&CompletionNotificationImpl, &CompletionNotificationFacade>(
                notification,
            )
        };
        cb(facade);
    }
}

/// We use a specialized wrapper here because we need to share it across threads (for wakers)
/// and do not want to go through the general primitive lifecycle management logic that we use
/// with `BoundPrimitive` etc because those actually assume presence of a runtime and will keep
/// alive the I/O driver - this is not desirable in our case because the user of the I/O driver
/// may want to keep wakers around until the I/O driver is dropped (which would be a cycle
/// because a `BoundPrimitive` keeps the I/O driver alive).
#[derive(Debug, Display)]
#[display("{primitive}")]
pub struct CompletionPort {
    primitive: PrimitiveImpl,
}

impl CompletionPort {
    const fn new(primitive: PrimitiveImpl) -> Self {
        Self { primitive }
    }

    pub fn as_handle(&self) -> &HANDLE {
        self.primitive.as_handle()
    }
}

impl Drop for CompletionPort {
    fn drop(&mut self) {
        self.primitive.close();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "Perfectly fine in test code")]

    use std::ptr;

    use mockall::Sequence;
    use windows::Win32::Foundation::{STATUS_BUFFER_TOO_SMALL, STATUS_SUCCESS};

    use super::*;
    use crate::pal::MockBindings;

    #[test]
    fn bind_primitive() {
        let mut bindings = MockBindings::new();

        let completion_port_handle = HANDLE(ptr::dangling_mut());

        // Closing this is outside scope of completion queue API, so we ignore closing it.
        let primitive_handle = HANDLE(ptr::dangling_mut());

        let mut seq = Sequence::new();

        // Ctor of CompletionQueueImpl.
        bindings
            .expect_create_io_completion_port()
            .times(1)
            .in_sequence(&mut seq)
            .withf(
                |file_handle,
                 existing_completion_port,
                 completion_key,
                 number_of_concurrent_threads| {
                    *file_handle == INVALID_HANDLE_VALUE
                        && existing_completion_port.is_none()
                        && *completion_key == 0
                        && *number_of_concurrent_threads == 1
                },
            )
            .return_const_st(Ok(completion_port_handle));

        // From bind().
        bindings
            .expect_create_io_completion_port()
            .times(1)
            .in_sequence(&mut seq)
            .withf_st(
                move |file_handle,
                      completion_port_handle,
                      completion_key,
                      number_of_concurrent_threads| {
                    *file_handle == primitive_handle
                        && *completion_port_handle == *completion_port_handle
                        && *completion_key == 0
                        && *number_of_concurrent_threads == 1
                },
            )
            .return_const_st(Ok(completion_port_handle));

        #[expect(clippy::cast_possible_truncation, reason = "constant known to fit")]
        let expected_file_completion_notification_mode =
            (FILE_SKIP_SET_EVENT_ON_HANDLE | FILE_SKIP_COMPLETION_PORT_ON_SUCCESS) as u8;

        // From configure_handle() call.
        bindings
            .expect_set_file_completion_notification_mode()
            .times(1)
            .in_sequence(&mut seq)
            .withf_st(move |file_handle, mode| {
                *file_handle == primitive_handle
                    && *mode == expected_file_completion_notification_mode
            })
            .return_const_st(Ok(()));

        bindings
            .expect_close_handle()
            .times(1)
            .withf_st(move |handle| *handle == completion_port_handle)
            .return_const_st(Ok(()));

        let bindings = BindingsFacade::from_mock(bindings);

        let completion_queue = CompletionQueueImpl::new(bindings.clone());

        // Closing this is outside scope of completion queue API, so we ignore closing it.
        let primitive = PrimitiveImpl::from_handle(primitive_handle, bindings);

        completion_queue.bind(&primitive.into()).unwrap();
    }

    #[cfg(not(miri))] // This takes like 3 minutes under Miri, so... let's not.
    #[test]
    #[expect(clippy::too_many_lines, reason = "test code")]
    fn poll() {
        // Various combinations of poll results. Note that we skip binding the primitive here, since
        // nothing in the implementation logic actually cares about primitives, the purpose of
        // binding is merely to configure the operating system to produce the notifications we want.

        let mut bindings = MockBindings::new();

        let completion_port_handle = HANDLE(ptr::dangling_mut());

        let mut seq = Sequence::new();

        // Ctor of CompletionQueueImpl.
        bindings
            .expect_create_io_completion_port()
            .times(1)
            .in_sequence(&mut seq)
            .withf(
                |file_handle,
                 existing_completion_port,
                 completion_key,
                 number_of_concurrent_threads| {
                    *file_handle == INVALID_HANDLE_VALUE
                        && existing_completion_port.is_none()
                        && *completion_key == 0
                        && *number_of_concurrent_threads == 1
                },
            )
            .return_const_st(Ok(completion_port_handle));

        // First poll (no timeout), successfully returns nothing.
        // Second poll (no timeout), returns 2 completion notifications (ok, ok).
        // Third poll (with timeout), returns nothing with timeout.
        // Fourth poll (with timeout), returns 2 completion notifications (err, ok).

        bindings
            .expect_get_queued_completion_status_ex()
            .times(1)
            .in_sequence(&mut seq)
            .returning(
                |_completion_port,
                 _completion_port_entries,
                 num_entries_removed,
                 _milliseconds,
                 _alertable| {
                    *num_entries_removed = 0;
                    Ok(())
                },
            );

        bindings
            .expect_get_queued_completion_status_ex()
            .times(1)
            .in_sequence(&mut seq)
            .returning(
                |_completion_port,
                 completion_port_entries,
                 num_entries_removed,
                 _milliseconds,
                 _alertable| {
                    assert!(completion_port_entries.len() >= 2);

                    // SAFETY: We asserted above that there is enough space.
                    unsafe {
                        completion_port_entries[0]
                            .as_mut_ptr()
                            .write(OVERLAPPED_ENTRY {
                                lpCompletionKey: 1111,
                                lpOverlapped: ptr::dangling_mut(),
                                Internal: STATUS_SUCCESS.0 as usize,
                                dwNumberOfBytesTransferred: 1111,
                            });
                    }
                    // SAFETY: We asserted above that there is enough space.
                    unsafe {
                        completion_port_entries[1]
                            .as_mut_ptr()
                            .write(OVERLAPPED_ENTRY {
                                lpCompletionKey: 2222,
                                lpOverlapped: ptr::dangling_mut(),
                                Internal: STATUS_SUCCESS.0 as usize,
                                dwNumberOfBytesTransferred: 2222,
                            });
                    }

                    *num_entries_removed = 2;
                    Ok(())
                },
            );

        bindings
            .expect_get_queued_completion_status_ex()
            .times(1)
            .in_sequence(&mut seq)
            .returning(
                |_completion_port,
                 _completion_port_entries,
                 num_entries_removed,
                 _milliseconds,
                 _alertable| {
                    *num_entries_removed = 0;
                    Err(windows::core::Error::from_hresult(HRESULT::from_win32(
                        WAIT_TIMEOUT.0,
                    )))
                },
            );

        bindings
            .expect_get_queued_completion_status_ex()
            .times(1)
            .in_sequence(&mut seq)
            .returning(
                |_completion_port,
                 completion_port_entries,
                 num_entries_removed,
                 _milliseconds,
                 _alertable| {
                    assert!(completion_port_entries.len() >= 2);

                    // SAFETY: We asserted above that there is enough space.
                    unsafe {
                        #[expect(clippy::cast_sign_loss, reason = "Win32 API says this is okay")]
                        completion_port_entries[0]
                            .as_mut_ptr()
                            .write(OVERLAPPED_ENTRY {
                                lpCompletionKey: 3333,
                                lpOverlapped: ptr::dangling_mut(),
                                // This is really just treated as a boolean, with
                                // GetLastError() used to get the real error code.
                                Internal: STATUS_BUFFER_TOO_SMALL.0 as usize,
                                dwNumberOfBytesTransferred: 3333,
                            });
                    }
                    // SAFETY: We asserted above that there is enough space.
                    unsafe {
                        completion_port_entries[1]
                            .as_mut_ptr()
                            .write(OVERLAPPED_ENTRY {
                                lpCompletionKey: 4444,
                                lpOverlapped: ptr::dangling_mut(),
                                Internal: STATUS_SUCCESS.0 as usize,
                                dwNumberOfBytesTransferred: 4444,
                            });
                    }

                    *num_entries_removed = 2;
                    Ok(())
                },
            );

        bindings
            .expect_close_handle()
            .times(1)
            .in_sequence(&mut seq)
            .withf_st(move |handle| *handle == completion_port_handle)
            .return_const_st(Ok(()));

        let bindings = BindingsFacade::from_mock(bindings);

        let mut completion_queue = CompletionQueueImpl::new(bindings);

        let results = get_completion_results(&mut completion_queue, 0);
        assert_eq!(results.len(), 0);

        let results = get_completion_results(&mut completion_queue, 0);
        assert_eq!(results.len(), 2);

        assert!(matches!(results[0], Ok(1111)));
        assert!(matches!(results[1], Ok(2222)));

        let results = get_completion_results(&mut completion_queue, 100);
        assert_eq!(results.len(), 0);

        let results = get_completion_results(&mut completion_queue, 100);
        assert_eq!(results.len(), 2);

        // Due to how GetLastError() works with Win32 error codes, this will return
        // "Error: operation successful" or something similar. That's fine,
        // as long as it is `Err(something)` we are happy.
        #[cfg(not(miri))] // Miri cannot handle Win32 errors.
        results[0].as_ref().unwrap_err();
        assert!(matches!(results[1], Ok(4444)));
    }

    fn get_completion_results(
        queue: &mut CompletionQueueImpl,
        max_wait_time_millis: u32,
    ) -> Vec<crate::Result<u32>> {
        let mut results = Vec::new();

        queue.process_completions(max_wait_time_millis, |notification| {
            results.push(notification.result());
        });

        results
    }
}