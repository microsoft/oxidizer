// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A unit of work to be executed on a worker thread.
pub struct WorkItem {
    data: *mut (),
    vtable_fn: fn(*mut ()),
}

impl WorkItem {
    /// Creates a new `WorkItem`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that, when `vtable_fn(data)` is invoked on
    /// the worker thread:
    ///
    /// - `data` is a valid pointer for whatever access `vtable_fn` performs,
    /// - `data` remains valid until `vtable_fn` returns,
    /// - `vtable_fn` does not unwind (or its unwinding is internally caught),
    /// - any cross-thread aliasing/`Send`/`Sync` requirements of the pointee
    ///   are satisfied.
    ///
    /// In practice the only sound use of this function is from
    /// `#[thunk]`-generated code.
    pub unsafe fn new(data: *mut (), vtable_fn: fn(*mut ())) -> Self {
        Self { data, vtable_fn }
    }

    /// Returns the pointer to the task data.
    #[cfg(test)]
    pub(crate) fn data(&self) -> *mut () {
        self.data
    }

    /// Returns the function pointer that executes the task.
    #[cfg(test)]
    pub(crate) fn vtable_fn(&self) -> fn(*mut ()) {
        self.vtable_fn
    }

    /// Executes the work item by invoking the function pointer with the data pointer.
    pub(crate) fn execute(self) {
        (self.vtable_fn)(self.data);
    }
}

impl core::fmt::Debug for WorkItem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WorkItem").finish_non_exhaustive()
    }
}

// SAFETY: WorkItem is sent across threads by design; the caller guarantees
// that the pointed-to data remains valid until the vtable_fn completes.
unsafe impl Send for WorkItem {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn dummy_vtable(ptr: *mut ()) {
        // SAFETY: In tests we pass a valid *mut u32.
        unsafe { *ptr.cast::<u32>() = 42 };
    }

    #[test]
    fn new_and_accessors() {
        let mut val: u32 = 0;
        let ptr = (&raw mut val).cast::<()>();
        // SAFETY: `ptr` points to a live `u32`; `dummy_vtable` writes a `u32`.
        let item = unsafe { WorkItem::new(ptr, dummy_vtable) };
        assert_eq!(item.data(), ptr);
        // Verify vtable_fn returns a callable function pointer.
        let vt = item.vtable_fn();
        let mut check: u32 = 0;
        vt((&raw mut check).cast::<()>());
        assert_eq!(check, 42);
    }

    #[test]
    fn execute_invokes_vtable_fn() {
        let mut val: u32 = 0;
        let ptr = (&raw mut val).cast::<()>();
        // SAFETY: `ptr` points to a live `u32`; `dummy_vtable` writes a `u32`.
        let item = unsafe { WorkItem::new(ptr, dummy_vtable) };
        item.execute();
        assert_eq!(val, 42);
    }

    #[test]
    fn debug_impl() {
        // SAFETY: `execute()` is never called on this instance, so the null
        // data pointer is never dereferenced.
        let item = unsafe { WorkItem::new(core::ptr::null_mut(), dummy_vtable) };
        let debug = format!("{item:?}");
        assert!(debug.contains("WorkItem"));
    }

    #[test]
    fn send_trait() {
        fn assert_send<T: Send>() {}
        assert_send::<WorkItem>();
    }
}
