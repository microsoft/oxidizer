// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This is an example of how one might implement some Windows write
//! operations on a `struct File` using the Oxidizer I/O subsystem.

#![cfg(windows)]
#![cfg(feature = "unstable-testing")]
#![cfg(not(miri))] // Miri cannot talk to real OS.

use std::error::Error;
use std::ffi::CString;
use std::fs;
use std::num::NonZeroU32;
use std::path::Path;

use bytes::BufMut;
use oxidizer_io::mem::Sequence;
use oxidizer_io::testing::with_io_test_harness;
use oxidizer_io::{AsNativePrimitiveExt, BeginResult, BoundPrimitive, Context, ReserveOptions};
use tempfile::NamedTempFile;
use windows::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateFileA, FILE_FLAG_OVERLAPPED, FILE_GENERIC_WRITE, FILE_SHARE_NONE,
    WriteFile,
};
use windows::core::PCSTR;

const FILE_SIZE: u64 = 1024 * 1024;

// We run this test using all the I/O models we support, to demonstrate:
// 1. That all of the I/O models work.
// 2. How you can write generic code that works with all of them.
#[test]
fn file_write_windows() -> Result<(), Box<dyn Error>> {
    with_io_test_harness(async move |io_context| {
        let target_file = NamedTempFile::new()?.into_temp_path();

        let file = file_create(&target_file, &io_context)?;
        file_fill_with_sample_data(&io_context, &file).await?;
        file.close().await;

        assert_file_looks_correct(&target_file);

        Ok(())
    })
}

// This is how you would implement a File::create() function using the Oxidizer I/O subsystem.
fn file_create(
    path: impl AsRef<Path>,
    io_context: &Context,
) -> oxidizer_io::Result<BoundPrimitive> {
    let path_cstr = CString::new(path.as_ref().to_str().unwrap()).unwrap();

    // SAFETY: No special safety requirements, just an FFI call.
    let handle = unsafe {
        CreateFileA(
            PCSTR::from_raw(path_cstr.as_ptr().cast()),
            FILE_GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            CREATE_ALWAYS,
            // FILE_FLAG_OVERLAPPED is required by `bind_primitive()` API contract.
            FILE_FLAG_OVERLAPPED,
            None,
        )
    }?;

    io_context.bind_primitive(handle)
}

// This is how you would implement a File::fill_with_zero() function using the Oxidizer I/O
// subsystem. Perhaps not a super useful function but you can use it as a reference for how to
// write any other data, as well (replace `put_bytes(77)` with some other content).
async fn file_fill_with_sample_data(
    io_context: &Context,
    file: &BoundPrimitive,
) -> oxidizer_io::Result<()> {
    // We write the file in pieces of at most this size (less if we run out of data to write).
    const MAX_WRITE_SIZE: NonZeroU32 = NonZeroU32::new(333_333).unwrap();

    let mut total_bytes_written: u64 = 0;

    // For write operations, it is guaranteed by the operating system that the operation will only
    // complete when all bytes have been written. This makes it easy to parallelize writes for
    // improved throughput, though we do not do it here to keep the example simple.
    while total_bytes_written < FILE_SIZE {
        // Note that the I/O subsystem may give us a bigger buffer than we requested. We use all
        // the memory it gives us, even if it gives more than we requested.
        let mut sequence_builder =
            io_context.reserve(MAX_WRITE_SIZE.get() as usize, ReserveOptions::default());
        debug_assert!(sequence_builder.capacity() >= MAX_WRITE_SIZE.get() as usize);

        let bytes_remaining_to_write = FILE_SIZE
            .checked_sub(total_bytes_written)
            .expect("somehow ended up with negative bytes remaining to write - impossible");
        let bytes_to_write = bytes_remaining_to_write.min(MAX_WRITE_SIZE.get().into());
        let bytes_to_write = bytes_to_write.min(sequence_builder.capacity() as u64);

        // The Windows API only accepts u32, so clamp it to u32 range.
        // A little silly because we are never requesting 4GB buffers here but let's be proper.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "we have clamped to safe range"
        )]
        let bytes_to_write = bytes_to_write.min(u32::MAX.into()) as u32;

        // Fill our buffer with some sample data.
        sequence_builder.put_bytes(77, bytes_to_write as usize);

        // Windows does support vectored file I/O but only if we read/write entire system
        // memory pages and filesystem sectors at once, bypassing the system caching logic.
        // This is entirely doable with sufficient effort but requires special consideration
        // such as a page-aligned memory allocator. While we can add support for page-
        // aligned memory allocation to our memory pool at a later date, we will skip this
        // for now to keep things simple, especially as filesystem I/O throughput is not a
        // critical bottleneck for us (for now).
        //
        // Instead, we fall back to non-vectored writes here and simply loop through all the
        // chunks of memory provided to us, writing them out sequentially. This is less efficient
        // than vectored I/O but still results in the correct data being written to the file.
        let sequence = sequence_builder.consume(bytes_to_write as usize);

        file_write(total_bytes_written, sequence, file).await?;

        total_bytes_written = total_bytes_written.saturating_add(bytes_to_write.into());
    }

    assert_eq!(total_bytes_written, FILE_SIZE);

    println!("Write completed. Total bytes: {total_bytes_written}.");

    Ok(())
}

fn assert_file_looks_correct(path: impl AsRef<Path>) {
    // Verify that the file is not just full of zeroes or some wrong data.
    let file_contents = fs::read(path).unwrap();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "manually verified to be in safe range"
    )]
    let expected_file_size = FILE_SIZE as usize;
    assert_eq!(file_contents.len(), expected_file_size);
    assert_eq!(*file_contents.first().expect("checked length"), 77);
}

// This is how you would implement a File::write() function using the Oxidizer I/O subsystem.
async fn file_write(
    offset: u64,
    bytes: Sequence,
    file: &BoundPrimitive,
) -> oxidizer_io::Result<()> {
    // In theory, it is possible for a Windows API write operation to only write a partial number
    // of bytes. In practice, this never seems to happen - writes only complete either when there
    // is an error or when everything is written. We will not bother implementing code to handle
    // the partial write case since it never seems to occur. A partial write appears to be widely
    // treated as equivalent to an error in practice, so we will do the same - if it should happen,
    // the I/O subsystem will automatically mark the operation as failed.
    file.write_bytes::<1>(bytes)
        .with_offset(offset)
        .begin(move |primitive, mut args| {
            // SAFETY: We are not allowed to reuse this for multiple calls and we are only
            // allowed to use it with the primitive given to this callback. We obey the rules.
            let overlapped = unsafe { args.overlapped() };

            let chunk = args
                .chunks()
                .next()
                .expect("a write with 0 chunks is not a legal operation - there must be one");

            // SAFETY: The buffer must remain valid for the duration of any asynchronous
            // I/O, which is guaranteed by the I/O subsystem that calls us.
            let result = unsafe {
                WriteFile(
                    *primitive.as_handle(),
                    Some(chunk),
                    Some(args.bytes_written_synchronously_as_mut()),
                    Some(overlapped),
                )
            };

            BeginResult::from_windows_result(result)
        })
        .await
}