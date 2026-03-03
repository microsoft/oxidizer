// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A unit of work to be executed on a worker thread.
pub struct WorkItem {
    data: *mut (),
    vtable_fn: fn(*mut ()),
}

impl WorkItem {
    /// Creates a new `WorkItem`.
    pub fn new(data: *mut (), vtable_fn: fn(*mut ())) -> Self {
        Self { data, vtable_fn }
    }

    /// Returns the pointer to the task data.
    #[must_use]
    pub fn data(&self) -> *mut () {
        self.data
    }

    /// Returns the function pointer that executes the task.
    #[must_use]
    pub fn vtable_fn(&self) -> fn(*mut ()) {
        self.vtable_fn
    }

    /// Executes the work item by invoking the function pointer with the data pointer.
    pub fn execute(self) {
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
        let item = WorkItem::new(ptr, dummy_vtable);
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
        let item = WorkItem::new(ptr, dummy_vtable);
        item.execute();
        assert_eq!(val, 42);
    }

    #[test]
    fn debug_impl() {
        let item = WorkItem::new(core::ptr::null_mut(), dummy_vtable);
        let debug = format!("{item:?}");
        assert!(debug.contains("WorkItem"));
    }

    #[test]
    fn send_trait() {
        fn assert_send<T: Send>() {}
        assert_send::<WorkItem>();
    }
}
