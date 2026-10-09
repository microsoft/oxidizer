// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "benchmark fixtures fail immediately on invalid controlled input")]

use std::ffi::{OsStr, OsString};
use std::hint::black_box;
use std::io::{self, Read, Write};

pub(crate) const fn initialize_native_fixture() {}

pub(crate) fn run(arguments: impl IntoIterator<Item = OsString>, prepared_state: &[u8]) -> i32 {
    let mut arguments = arguments.into_iter();
    let _program = arguments.next();
    match arguments.next().as_deref() {
        Some(command) if command == OsStr::new("entry") => write_entry_marker(),
        Some(command) if command == OsStr::new("hold") => hold_for_observation(prepared_state),
        Some(command) if command == OsStr::new("emit") => emit(arguments),
        first => {
            let mut checksum = prepared_checksum(prepared_state);
            if let Some(first) = first {
                checksum ^= hash_os_string(first);
            }
            for argument in arguments {
                checksum = checksum.rotate_left(7) ^ hash_os_string(&argument);
            }
            for (key, value) in std::env::vars_os() {
                checksum ^= hash_os_string(&key).wrapping_mul(31) ^ hash_os_string(&value);
            }
            black_box(checksum);
            0
        }
    }
}

fn write_entry_marker() -> i32 {
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"E").unwrap();
    stdout.flush().unwrap();
    0
}

fn hold_for_observation(prepared_state: &[u8]) -> i32 {
    black_box(prepared_checksum(prepared_state));
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"R").unwrap();
    stdout.flush().unwrap();
    let mut release = [0_u8; 1];
    io::stdin().lock().read_exact(&mut release).unwrap();
    black_box(release);
    0
}

fn emit(mut arguments: impl Iterator<Item = OsString>) -> i32 {
    let stdout_bytes = parse_size(arguments.next());
    let stderr_bytes = parse_size(arguments.next());
    write_repeated(io::stdout().lock(), b'o', stdout_bytes);
    write_repeated(io::stderr().lock(), b'e', stderr_bytes);
    0
}

fn parse_size(value: Option<OsString>) -> usize {
    value.unwrap().into_string().unwrap().parse().unwrap()
}

fn write_repeated(mut writer: impl Write, byte: u8, byte_count: usize) {
    let chunk = [byte; 16 * 1_024];
    let mut remaining = byte_count;
    while remaining != 0 {
        let count = remaining.min(chunk.len());
        writer.write_all(&chunk[..count]).unwrap();
        remaining -= count;
    }
    writer.flush().unwrap();
}

fn prepared_checksum(state: &[u8]) -> u64 {
    state
        .chunks(4_096)
        .fold(0_u64, |checksum, page| checksum.rotate_left(5) ^ u64::from(page[0]))
}

fn hash_os_string(value: &OsStr) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;

        value.as_bytes().iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        })
    }
    #[cfg(not(unix))]
    {
        value.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
        })
    }
}
