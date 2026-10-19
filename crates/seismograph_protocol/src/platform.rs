// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::*;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(not(any(unix, windows)))]
compile_error!("Seismograph local monitors require Unix or Windows");

#[derive(Debug)]
pub(crate) struct RegistryDirectory {
    pub(crate) path: std::path::PathBuf,
    pub(crate) file: std::fs::File,
    pub(crate) temporary: Option<tempfile::TempDir>,
}

#[derive(Debug)]
pub(crate) struct DirectoryEntry {
    pub(crate) path: std::path::PathBuf,
    pub(crate) owner: crate::monitor::MonitorOwner,
}

pub(crate) type DirectoryEntries = Vec<std::io::Result<DirectoryEntry>>;

#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Default)]
pub(crate) struct PendingReads(std::sync::atomic::AtomicUsize);

#[cfg(any(test, feature = "test-util"))]
impl PendingReads {
    pub(crate) fn enter(&self) -> PendingRead<'_> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Release);
        PendingRead(self)
    }

    pub(crate) fn active(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire) != 0
    }
}

#[cfg(any(test, feature = "test-util"))]
pub(crate) struct PendingRead<'a>(&'a PendingReads);

#[cfg(any(test, feature = "test-util"))]
impl Drop for PendingRead<'_> {
    fn drop(&mut self) {
        self.0.0.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn pending_read_observations_follow_all_guard_lifetimes() {
        let reads = PendingReads::default();
        assert!(!reads.active());
        let first = reads.enter();
        let second = reads.enter();
        assert!(reads.active());
        drop(first);
        assert!(reads.active());
        drop(second);
        assert!(!reads.active());
    }
}
