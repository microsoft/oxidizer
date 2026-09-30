// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises the public proc-macro shims without a dependency cycle back to Arty.

mod fixture {
    use std::cell::Cell;

    thread_local! {
        pub(super) static FAIL_CONSTRUCTION: Cell<bool> = const { Cell::new(false) };
    }

    #[derive(Debug)]
    pub(super) struct Builtins(pub(super) usize);

    #[derive(Debug)]
    pub(super) struct Runtime {
        value: usize,
    }

    impl Runtime {
        pub(super) fn new() -> Result<Self, &'static str> {
            if FAIL_CONSTRUCTION.replace(false) {
                Err("forced construction failure")
            } else {
                Ok(Self { value: 41 })
            }
        }

        pub(super) fn run<F, Fut, R>(self, factory: F) -> R
        where
            F: FnOnce(Builtins) -> Fut,
            Fut: Future<Output = R>,
        {
            futures::executor::block_on(factory(Builtins(self.value)))
        }
    }
}

#[arty_macros::main(runtime_path = crate::fixture)]
async fn entrypoint(cx: fixture::Builtins) -> usize {
    cx.0.saturating_add(1)
}

#[arty_macros::test(runtime_path = crate::fixture)]
async fn runtime_test(cx: fixture::Builtins) {
    assert_eq!(cx.0, 41);
}

trait HasContext {
    type Context;
}

struct App;

impl HasContext for App {
    type Context = fixture::Builtins;
}

#[arty_macros::main(runtime_path = crate::fixture)]
async fn qualified_main(cx: <App as HasContext>::Context) -> usize {
    cx.0
}

#[arty_macros::test(runtime_path = crate::fixture)]
async fn qualified_test(cx: <App as HasContext>::Context) {
    assert_eq!(cx.0, 41);
}

#[test]
fn expanded_entry_points_exist_and_execute() {
    assert_eq!(entrypoint(), 42);
    runtime_test();
    assert_eq!(qualified_main(), 41);
    qualified_test();
}

#[test]
#[should_panic(expected = "forced construction failure")]
fn constructor_failure_is_not_discarded() {
    fixture::FAIL_CONSTRUCTION.set(true);
    let _ = entrypoint();
}
