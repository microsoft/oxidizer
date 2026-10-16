// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unit-test-only fakes stay unavailable in normal builds, even with `test-util`.

#[fakeable::fakeable(fake_impl = fakes::FakeService, fakes_feature = false)]
#[derive(Clone, Debug)]
pub struct Service {
    value: u32,
}

#[fakeable::fakeable(fakes_feature = false)]
impl Service {
    pub const fn new(value: u32) -> Self {
        Self { value }
    }

    pub fn value(&self) -> u32 {
        self.value
    }

    #[must_use]
    pub fn with_value(&self, value: u32) -> Self {
        Self { value }
    }
}

#[cfg(test)]
pub mod fakes {
    use std::cell::Cell;
    use std::rc::Rc;

    /// Shared state exercises clone and `Self` forwarding. Deliberately not `Send` or `Sync`,
    /// so the dependency-build assertions detect fake storage leaking into the real-only wrapper.
    #[derive(Clone, Debug)]
    pub struct FakeService {
        pub value: Rc<Cell<u32>>,
    }

    impl FakeService {
        #[must_use]
        pub fn value(&self) -> u32 {
            self.value.get()
        }

        #[must_use]
        pub fn with_value(&self, value: u32) -> Self {
            self.value.set(value);
            self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::rc::Rc;

    use static_assertions::assert_not_impl_any;

    use super::Service;
    use super::fakes::FakeService;

    // Shared interior mutability must stay visible in the fake-enabled unwind contract.
    assert_not_impl_any!(Service: UnwindSafe, RefUnwindSafe);

    #[test]
    fn test_only_fake_preserves_shared_state_and_self_returns() {
        let value = Rc::new(Cell::new(7));
        let service = Service::fake(FakeService { value: Rc::clone(&value) });
        let clone = service.clone();

        assert_eq!(service.value(), 7);
        assert_eq!(service.with_value(9).value(), 9);
        assert_eq!(clone.value(), 9);
        assert_eq!(value.get(), 9);
    }
}
