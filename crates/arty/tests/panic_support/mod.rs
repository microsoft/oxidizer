// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime};

pub(crate) fn runtime() -> Runtime {
    Runtime::builder()
        .processor_count(ProcessorCount::exactly(1))
        .blocking_pool_policy(BlockingPoolPolicy::shared(1))
        .build()
        .unwrap()
}

pub(crate) fn isolated(name: &str, body: fn()) {
    #[cfg(miri)]
    {
        let _ = name;
        body();
    }
    #[cfg(not(miri))]
    {
        use std::process::{Command, Stdio};
        use std::time::Instant;

        const CHILD: &str = "ARTY_PANIC_CONTRACT_CHILD";
        if std::env::var(CHILD).as_deref() == Ok(name) {
            body();
            return;
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, name)
            .env("RUST_BACKTRACE", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + testing_aids::TEST_TIMEOUT * 3;
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "{name} exceeded its child-process deadline\nstdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{name} failed in its child: {}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
