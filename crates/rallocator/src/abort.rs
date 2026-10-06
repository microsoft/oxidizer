// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub(crate) fn abort() -> ! {
    #[cfg(all(test, coverage_nightly))]
    {
        unsafe extern "C" {
            fn __llvm_profile_write_file() -> std::ffi::c_int;
        }
        // Abort does not run the coverage runtime's normal exit handler.
        // SAFETY: Instrumented test binaries link this runtime entrypoint;
        // fatal-path subprocesses call it synchronously before genuine termination.
        let _profile_status = unsafe { __llvm_profile_write_file() };
    }
    std::process::abort()
}

pub(crate) fn require(success: bool) {
    if !success {
        abort();
    }
}

#[cfg(test)]
pub(crate) fn assert_aborts(name: &str, action: impl FnOnce()) {
    const CASE: &str = "RALLOCATOR_ABORT_CASE";
    const BASELINE: &str = "RALLOCATOR_ABORT_BASELINE";
    if std::env::var_os(CASE).as_deref() == Some(std::ffi::OsStr::new(name)) {
        #[cfg(target_os = "linux")]
        {
            let limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            // SAFETY: RLIMIT_CORE accepts this live limits structure; only this subprocess is changed.
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const limit) }, 0);
        }
        if std::env::var_os(BASELINE).is_some() {
            abort();
        }
        action();
        return;
    }
    let executable = std::env::current_exe().unwrap();
    let baseline = std::process::Command::new(&executable)
        .args(["--exact", name])
        .env(CASE, name)
        .env(BASELINE, "1")
        .output()
        .unwrap();
    assert!(!baseline.status.success());
    let actual = std::process::Command::new(executable)
        .args(["--exact", name])
        .env(CASE, name)
        .env_remove(BASELINE)
        .output()
        .unwrap();
    assert_eq!(
        actual.status, baseline.status,
        "fatal path must terminate exactly like process::abort"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprocess_oracle_rejects_normal_return() {
        let outcome = std::panic::catch_unwind(|| assert_aborts("abort::tests::subprocess_oracle_rejects_normal_return", || {}));
        if std::env::var_os("RALLOCATOR_ABORT_CASE").is_some() {
            assert!(outcome.is_ok());
        } else {
            assert!(outcome.is_err());
        }
    }

    #[test]
    fn subprocess_oracle_accepts_genuine_abort() {
        assert_aborts("abort::tests::subprocess_oracle_accepts_genuine_abort", || abort());
        require(true);
    }

    #[test]
    fn failed_invariant_aborts_without_unwinding() {
        assert_aborts("abort::tests::failed_invariant_aborts_without_unwinding", || require(false));
    }
}
