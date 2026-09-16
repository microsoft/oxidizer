// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stack-only Linux pressure sampling. Standard procfs and cgroup-v2 mounts are
//! best-effort hints; custom mounts and cgroup-v1 limits are not discovered.

use std::ffi::CStr;

use super::super::MemoryStatus;

pub(crate) fn memory_status() -> Option<MemoryStatus> {
    let mut contents = [0_u8; 4096];
    let length = read_file(c"/proc/meminfo", &mut contents)?;
    let contents = &contents[..length];
    let field = |name: &[u8]| {
        contents
            .split(|byte| *byte == b'\n')
            .find_map(|line| number(line.strip_prefix(name)?).and_then(|value| value.checked_mul(1024)))
    };
    let mut status = MemoryStatus {
        total: field(b"MemTotal:")?,
        available: field(b"MemAvailable:")?,
    };
    apply_cgroup(b"/sys/fs/cgroup", &mut status);

    let mut membership = [0_u8; 512];
    if let Some(length) = read_file(c"/proc/self/cgroup", &mut membership)
        && let Some(relative) = membership[..length]
            .split(|byte| *byte == b'\n')
            .find_map(|line| line.strip_prefix(b"0::"))
        && relative.starts_with(b"/")
    {
        let mut path = [0_u8; 768];
        let root = b"/sys/fs/cgroup";
        path[..root.len()].copy_from_slice(root);
        path[root.len()..root.len() + relative.len()].copy_from_slice(relative);
        let mut length = root.len() + relative.len();
        // Read parent limits as well: a leaf's "max" does not remove an ancestor cap.
        for _ in 0..32 {
            apply_cgroup(&path[..length], &mut status);
            let Some(separator) = path[..length].iter().rposition(|byte| *byte == b'/') else {
                break;
            };
            if separator <= root.len() {
                break;
            }
            length = separator;
        }
    }
    Some(status)
}

fn apply_cgroup(directory: &[u8], status: &mut MemoryStatus) {
    let read_value = |file: &[u8]| {
        let mut path = [0_u8; 800];
        let length = directory.len().checked_add(file.len())?;
        if length >= path.len() {
            return None;
        }
        path[..directory.len()].copy_from_slice(directory);
        path[directory.len()..length].copy_from_slice(file);
        let path = CStr::from_bytes_with_nul(&path[..=length]).ok()?;
        let mut contents = [0_u8; 64];
        let length = read_file(path, &mut contents)?;
        number(&contents[..length])
    };
    if let (Some(limit), Some(used)) = (read_value(b"/memory.max"), read_value(b"/memory.current")) {
        status.total = status.total.min(limit);
        status.available = status.available.min(limit.saturating_sub(used));
    }
}

fn number(bytes: &[u8]) -> Option<usize> {
    let mut value = 0_usize;
    let mut found = false;
    for &byte in bytes.iter().skip_while(|byte| byte.is_ascii_whitespace()) {
        if !byte.is_ascii_digit() {
            break;
        }
        found = true;
        value = value.checked_mul(10)?.checked_add(usize::from(byte - b'0'))?;
    }
    found.then_some(value)
}

fn read_file(path: &CStr, output: &mut [u8]) -> Option<usize> {
    // SAFETY: a NUL-terminated path and a live writable buffer are supplied.
    // Direct syscalls avoid reentrant allocation through filesystem abstractions.
    unsafe {
        let descriptor = libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        if descriptor < 0 {
            return None;
        }
        let length = libc::read(descriptor, output.as_mut_ptr().cast(), output.len());
        libc::close(descriptor);
        usize::try_from(length).ok().filter(|length| *length < output.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_accept_proc_units_and_reject_unlimited_or_overflow() {
        assert_eq!(number(b"  1234 kB"), Some(1234));
        assert_eq!(number(b"max\n"), None);
        assert_eq!(number(b"9999999999999999999999999"), None);
    }

    #[test]
    fn host_memory_query_reports_consistent_bounds() {
        let memory = memory_status().unwrap();
        assert!(memory.total > 0);
        assert!(memory.available <= memory.total);
    }
}
