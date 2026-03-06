use std::sync::Mutex;

use type_map::concurrent::TypeMap;

/// A thread-safe, append-only type map with interior mutability.
///
/// Values are stored in `Box<dyn Any + Send + Sync>` (via [`TypeMap`]). Once inserted,
/// entries are never removed or overwritten, which allows returning references with the
/// lifetime of `&self` rather than a lock guard.
pub(crate) struct SharedTypeMap {
    inner: Mutex<TypeMap>,
}

impl SharedTypeMap {
    pub(crate) fn from_type_map(types: TypeMap) -> Self {
        SharedTypeMap { inner: Mutex::new(types) }
    }

    pub(crate) fn contains<O: Send + Sync + 'static>(&self) -> bool {
        let guard = self.inner.lock().expect("SharedTypeMap mutex poisoned");
        guard.get::<O>().is_some()
    }

    pub(crate) fn try_get<O: Send + Sync + 'static>(&self) -> Option<&O> {
        let guard = self.inner.lock().expect("SharedTypeMap mutex poisoned");
        let ptr = guard.get::<O>().map(|r| r as *const O);
        drop(guard);
        // SAFETY: TypeMap stores values in Box<dyn Any + Send + Sync>. The Box's heap
        // allocation is not moved by HashMap resizing — only the fat pointer stored as a
        // HashMap value is relocated. This SharedTypeMap is append-only: entries are never
        // removed or overwritten (all mutations use entry-or-insert semantics). The heap
        // allocation therefore lives for the lifetime of the SharedTypeMap (&self). The
        // Mutex ensures no data races while the raw pointer is obtained.
        ptr.map(|p| unsafe { &*p })
    }

    /// Inserts a value if not already present and returns a reference to the stored value.
    ///
    /// Uses entry-or-insert semantics: if a value of type `O` is already present,
    /// the existing value is kept and the new value is dropped.
    pub(crate) fn get_or_insert<O: Send + Sync + 'static>(&self, value: O) -> &O {
        let mut guard = self.inner.lock().expect("SharedTypeMap mutex poisoned");
        let reference = guard.entry::<O>().or_insert(value);
        let ptr: *const O = reference;
        drop(guard);
        // SAFETY: same as try_get — append-only invariant ensures stable heap addresses.
        unsafe { &*ptr }
    }
}
