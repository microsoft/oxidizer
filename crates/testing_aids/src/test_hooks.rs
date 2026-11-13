// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;

thread_local! {
    static MODE: Cell<Option<bool>> = const { Cell::new(None) };
    static BACKGROUND: Cell<Option<usize>> = const { Cell::new(None) };
    static WATCHDOG: Cell<Option<usize>> = const { Cell::new(None) };
    static EXIT: Cell<bool> = const { Cell::new(false) };
}

pub(crate) struct ExitRequested(pub(crate) i32);

pub(crate) fn mode() -> Option<bool> {
    MODE.get()
}

pub(crate) fn with_mode(mode: bool, run: impl FnOnce()) {
    struct Reset(Option<bool>);
    impl Drop for Reset {
        fn drop(&mut self) {
            MODE.set(self.0);
        }
    }
    let _reset = Reset(MODE.replace(Some(mode)));
    run();
}

pub(crate) fn background_requested() -> bool {
    if let Some(count) = BACKGROUND.get() {
        BACKGROUND.set(Some(count + 1));
        true
    } else {
        false
    }
}

pub(crate) fn with_background(run: impl FnOnce()) -> usize {
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) {
            BACKGROUND.set(self.0);
        }
    }
    let _reset = Reset(BACKGROUND.replace(Some(0)));
    run();
    BACKGROUND.get().unwrap()
}

pub(crate) fn watchdog_is_injected() -> bool {
    WATCHDOG.get().is_some()
}

pub(crate) fn record_watchdog_join() {
    WATCHDOG.set(Some(WATCHDOG.get().unwrap() + 1));
}

pub(crate) fn with_watchdog(run: impl FnOnce()) -> usize {
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) {
            WATCHDOG.set(self.0);
        }
    }
    let _reset = Reset(WATCHDOG.replace(Some(0)));
    run();
    WATCHDOG.get().unwrap()
}

#[expect(
    clippy::exit,
    reason = "without an exit probe the watchdog must retain its native process-termination behavior"
)]
pub(crate) fn exit(code: i32) -> ! {
    if EXIT.get() {
        std::panic::panic_any(ExitRequested(code));
    }
    std::process::exit(code)
}

pub(crate) fn with_exit_probe(run: impl FnOnce()) -> i32 {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            EXIT.set(self.0);
        }
    }
    let _reset = Reset(EXIT.replace(true));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    outcome.unwrap_err().downcast::<ExitRequested>().unwrap().0
}
