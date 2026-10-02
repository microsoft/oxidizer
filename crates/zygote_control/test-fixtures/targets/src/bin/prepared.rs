// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use zygote_rt::{Launch, Prepared, ZygoteSafe};

struct State {
    prepared_by: u32,
    #[cfg(target_os = "linux")]
    _alternate_stack: Box<[u8]>,
}

// SAFETY: the fixture state contains immutable owned bytes and an integer
// captured before forking; the alternate stack allocation remains live.
unsafe impl ZygoteSafe for State {}

#[cfg(target_os = "linux")]
unsafe extern "C" fn prepared_signal_handler(_signal: libc::c_int) {}

fn prepare() -> Result<Prepared<State>, String> {
    if std::env::var_os("ZYGOTE_TEST_PREPARE_FAIL").is_some() {
        return Err("requested fixture failure".to_owned());
    }
    #[cfg(target_os = "linux")]
    let alternate_stack = unsafe {
        // SAFETY: the fixture is single-threaded during preparation and
        // installs valid process-local signal state for regression probes.
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = prepared_signal_handler as *const () as usize;
        if libc::sigemptyset(&raw mut action.sa_mask) != 0 || libc::sigaction(libc::SIGUSR1, &raw const action, std::ptr::null_mut()) != 0 {
            return Err("failed to install prepared signal handler".to_owned());
        }
        let mut stack = vec![0u8; libc::SIGSTKSZ].into_boxed_slice();
        let alternate = libc::stack_t {
            ss_sp: stack.as_mut_ptr().cast(),
            ss_flags: 0,
            ss_size: stack.len(),
        };
        if libc::sigaltstack(&raw const alternate, std::ptr::null_mut()) != 0 {
            return Err("failed to install prepared alternate signal stack".to_owned());
        }
        stack
    };
    let state = State {
        prepared_by: std::process::id(),
        #[cfg(target_os = "linux")]
        _alternate_stack: alternate_stack,
    };
    #[cfg(target_os = "linux")]
    {
        Prepared::protected(state).map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(Prepared::new(state))
    }
}

fn application(state: &'static State, launch: Launch<'_>) -> i32 {
    zygote_test_targets::run(launch.into_args_os(), Some(state.prepared_by))
}

zygote_rt::prepared_main!(prepare, application);
