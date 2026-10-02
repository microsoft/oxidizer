// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::error::Error;

use zygote_control::Zygote;

fn main() -> Result<(), Box<dyn Error>> {
    let target = std::env::args_os().nth(1).ok_or("usage: controller TARGET")?;
    #[cfg(target_os = "linux")]
    let close_stdin = std::env::var_os("ZYGOTE_TEST_CLOSE_STDIN");
    #[cfg(target_os = "linux")]
    if close_stdin.as_deref() == Some(std::ffi::OsStr::new("before-init")) {
        // SAFETY: this single-threaded fixture intentionally exercises worker
        // startup behavior when the controller has no standard input.
        unsafe {
            libc::close(libc::STDIN_FILENO);
        }
    }
    let zygote = Zygote::builder(target).spawn()?;
    #[cfg(target_os = "linux")]
    if close_stdin.is_some() {
        // SAFETY: this single-threaded fixture intentionally exercises launch
        // behavior when the controller has no standard input descriptor.
        if close_stdin.as_deref() != Some(std::ffi::OsStr::new("before-init")) {
            unsafe {
                libc::close(libc::STDIN_FILENO);
            }
        }
        let output = zygote
            .command()
            .arg("stdin-state")
            .stdout(zygote_control::Stdio::piped())
            .stderr(zygote_control::Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        assert!(output.status.success());
        print!("{}", String::from_utf8(output.stdout)?);
        return Ok(());
    }
    let output = zygote
        .command()
        .args(["report", "example"])
        .env("ZYGOTE_TEST_VALUE", "documented")
        .output()?;
    assert!(output.status.success());
    print!("{}", String::from_utf8(output.stdout)?);

    let launcher = zygote.launcher();
    let workers = ["concurrent-one", "concurrent-two"].map(|argument| {
        let launcher = launcher.clone();
        std::thread::spawn(move || {
            launcher
                .command()
                .args(["report", argument])
                .env("ZYGOTE_TEST_VALUE", argument)
                .output()
        })
    });
    for worker in workers {
        let output = worker.join().map_err(|_panic| "launch thread panicked")??;
        assert!(output.status.success());
        print!("{}", String::from_utf8(output.stdout)?);
    }
    Ok(())
}
