// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This is an example of how one might implement some Windows read
//! operations on a `struct File` using the Oxidizer I/O subsystem.

#![cfg(windows)]
#![cfg(feature = "unstable-testing")]
#![cfg(not(miri))] // Miri cannot talk to real OS.

use std::error::Error;
use std::ffi::CString;
use std::fs;
use std::num::NonZeroU32;
use std::path::Path;

use bytes::Buf;
use oxidizer_io::mem::Sequence;
use oxidizer_io::testing::with_io_test_harness;
use oxidizer_io::{AsNativePrimitiveExt, BeginResult, BoundPrimitive, Context, ReserveOptions};
use tempfile::NamedTempFile;
use windows::Win32::Storage::FileSystem::{
    CreateFileA, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING,
};
use windows::core::{BOOL, PCSTR};
use windows_sys::Win32::Storage::FileSystem::ReadFile;

const FILE_SIZE: u64 = 1024 * 1024;

// We run this test using all the I/O models we support, to demonstrate:
// 1. That all of the I/O models work.
// 2. How you can write generic code that works with all of them.
#[test]
fn file_read_windows() -> Result<(), Box<dyn Error>> {
    with_io_test_harness(async move |io_context| {
        let target_file = NamedTempFile::new()?.into_temp_path();

        // This is the test test data file to read from. Just a bunch of 66s.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "manually verified to be in safe range"
        )]
        fs::write(&target_file, vec![66u8; FILE_SIZE as usize]).unwrap();

        let file = file_open(target_file, &io_context)?;
        file_read_to_end(&io_context, file).await?;

        Ok(())
    })
}

// This is how you would implement a File::open() function using the Oxidizer I/O subsystem.
fn file_open(path: impl AsRef<Path>, io_context: &Context) -> oxidizer_io::Result<BoundPrimitive> {
    let path_cstr = CString::new(path.as_ref().to_str().unwrap()).unwrap();

    // SAFETY: No special safety requirements, just an FFI call.
    let handle = unsafe {
        CreateFileA(
            PCSTR::from_raw(path_cstr.as_ptr().cast()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ,
            None,
            OPEN_EXISTING,
            // FILE_FLAG_OVERLAPPED is required by `bind_primitive()` API contract.
            FILE_FLAG_OVERLAPPED,
            None,
        )
    }?;

    io_context.bind_primitive(handle)
}

// This is how you would implement a File::read_to_end() function using the Oxidizer I/O subsystem.
async fn file_read_to_end(io_context: &Context, file: BoundPrimitive) -> oxidizer_io::Result<()> {
    // We read the file in pieces of at most this size.
    //
    // There is no general guarantee that a read from a file handle will return exactly the
    // amount of bytes requested - they might not be available yet (e.g. if it represents
    // a pipe), in which case it returns whatever it has available. This is not necessarily
    // a problem if reading files from disk where the files already exist in full but this is
    // a bit of a special case, so in favor of having a general purpose example, we instead do
    // a "probing" read that reads piece by piece until end of file, so that the logic would also
    // work for other types of stream-based handles.
    //
    // In principle, it may be possible to also do parallel reads where different parts of a
    // file are read concurrently, to achieve higher throughput (assuming the file is not
    // being created during the write, which is a safe assumption at least in this case).
    // Alternatively, vectored reads from disk may offer even better performance but have
    // complex terms and conditions attached (out of scope of this example). We will keep it
    // simple and sequential here, though, as this is just a basic example.
    const MAX_READ_SIZE: NonZeroU32 = NonZeroU32::new(1024 * 1024).unwrap();

    let mut total_bytes_read: u64 = 0;

    while total_bytes_read < FILE_SIZE {
        let mut bytes_read = file_read(total_bytes_read, MAX_READ_SIZE, io_context, &file).await?;

        if bytes_read.is_empty() {
            // We might reach EOF before we expected.
            println!(
                "Read completed because we reached EOF prematurely. Total bytes: {total_bytes_read}."
            );
            return Ok(());
        }

        total_bytes_read = total_bytes_read.saturating_add(bytes_read.len() as u64);

        // Just to check that we got the expected test data, not a bunch of zero bytes or similar.
        assert_eq!(bytes_read.get_u8(), 66);
    }

    assert_eq!(total_bytes_read, FILE_SIZE);

    println!("Read completed. Total bytes: {total_bytes_read}.");
    Ok(())
}

// This is how you would implement a File::read() function using the Oxidizer I/O subsystem.
async fn file_read(
    offset: u64,
    bytes_to_read: NonZeroU32,
    io_context: &Context,
    file: &BoundPrimitive,
) -> oxidizer_io::Result<Sequence> {
    let sequence_builder =
        io_context.reserve(bytes_to_read.get() as usize, ReserveOptions::default());
    debug_assert!(sequence_builder.capacity() >= bytes_to_read.get() as usize);

    // Note that the SequenceBuilder we got may contain more capacity than we need. We may
    // choose to make use of this free extra capacity if we want to. We do.

    let operation =
        file.read_bytes(sequence_builder)
            .with_offset(offset)
            .begin(move |primitive, mut args| {
                // Windows does support vectored file I/O but only if we read/write entire system
                // memory pages and filesystem sectors at once, bypassing the system caching logic.
                // This is entirely doable with sufficient effort but requires special consideration
                // such as a page-aligned memory allocator. While we can add support for page-
                // aligned memory allocation to our memory pool at a later date, we will skip this
                // for now to keep things simple, especially as filesystem I/O throughput is not a
                // critical bottleneck for us (for now).
                //
                // Instead, we issue non-vectored reads here, which means we only fill the first
                // chunk of the SequenceBuilder with data - any chunks beyond the first are not
                // usable by us. This is the standard pattern for I/O endpoints that do not support
                // vectored I/O, though it is not universally applicable (e.g. partial read of UDP
                // packet is just an error) so is a choice the I/O endpoint implementation must make
                // for itself.
                //
                // This is the first chunk. Note that it may even be larger than the requested size
                // because the memory pool is free to give us more capacity than we need. We just
                // try make the best use of the first chunk here.
                let chunk = args.iter_chunks().next().expect(
                "I/O subsystem issued us a SequenceBuilder with no capacity - should be impossible",
            );

                let bytes_remaining_to_read = FILE_SIZE
                    .checked_sub(offset)
                    .expect("somehow ended up with negative bytes remaining to read - impossible");
                let bytes_to_read = bytes_remaining_to_read.min(chunk.len() as u64);

                // The Windows API only accepts u32, so clamp it to u32 range.
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "we have clamped to safe range"
                )]
                let bytes_to_read = bytes_to_read.min(u32::MAX.into()) as u32;

                // We are using ReadFile from windows-sys because the one from windows requires
                // the buffer to be initialized. This also requires us to pass the OVERLAPPED
                // from windows-sys, which we can just cast here as it has the same memory layout.
                // https://github.com/microsoft/windows-rs/issues/2106

                // SAFETY: We are not allowed to reuse this for multiple calls and we are only
                // allowed to use it with the primitive given to this callback. We obey the rules.
                #[expect(
                    clippy::absolute_paths,
                    reason = "intentionally being very explicit for readability"
                )]
                let overlapped_sys = unsafe {
                    args.overlapped()
                        .cast::<windows_sys::Win32::System::IO::OVERLAPPED>()
                };

                // SAFETY: The buffer must remain valid for the duration of any asynchronous
                // I/O, which is guaranteed by the I/O subsystem that calls us.
                let result_bool = unsafe {
                    ReadFile(
                        primitive.as_handle().0,
                        chunk.as_mut_ptr().cast(),
                        bytes_to_read,
                        &raw mut *args.bytes_read_synchronously_as_mut(),
                        overlapped_sys,
                    )
                };

                BeginResult::from_bool(BOOL(result_bool))
            });

    let (_bytes_read, mut bytes) = operation.await?;
    Ok(bytes.consume_all())
}