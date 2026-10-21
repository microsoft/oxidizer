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

// Moving this owner never retags the pointee while native I/O retains its pointer.
#[cfg(any(windows, test))]
#[derive(Debug)]
pub(super) struct NativeAllocation<T>(std::ptr::NonNull<T>);

#[cfg(any(windows, test))]
impl<T> NativeAllocation<T> {
    pub(super) fn new(value: T) -> Self {
        Self(std::ptr::NonNull::new(Box::into_raw(Box::new(value))).expect("Box::into_raw returns a non-null allocation"))
    }

    pub(super) const fn as_ptr(&self) -> *mut T {
        self.0.as_ptr()
    }
}

#[cfg(any(windows, test))]
impl<T> Drop for NativeAllocation<T> {
    fn drop(&mut self) {
        // SAFETY: new transferred this allocation's sole ownership. Native users
        // must finish or cancel and drain I/O before dropping its owner.
        drop(unsafe { Box::from_raw(self.0.as_ptr()) });
    }
}

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
    fn native_allocation_pointer_survives_owner_moves_and_repeated_access() {
        let allocation = NativeAllocation::new([0_u8; 8]);
        let original = allocation.as_ptr();
        let mut container = Some(allocation);
        let moved = std::hint::black_box(container.take().unwrap());
        assert_eq!(moved.as_ptr(), original);
        // SAFETY: both pointers refer to the same live allocation. No pointee
        // references exist, including across the owner move and accessor call.
        unsafe { original.write(*b"complete") };
        // SAFETY: the raw write completed and the allocation is still owned.
        assert_eq!(unsafe { moved.as_ptr().read() }, *b"complete");
    }

    #[test]
    fn native_allocation_drops_its_value_exactly_once() {
        struct Value<'a>(&'a std::cell::Cell<u32>);
        impl Drop for Value<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = std::cell::Cell::new(0);
        let allocation = NativeAllocation::new(Value(&drops));
        drop(allocation);
        assert_eq!(drops.get(), 1);
    }

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
