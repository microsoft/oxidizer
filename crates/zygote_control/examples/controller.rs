// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Launches a zygote-enabled target and prints one captured response.

use std::error::Error;

use zygote_control::Zygote;

fn main() -> Result<(), Box<dyn Error>> {
    let target = std::env::args_os().nth(1).ok_or("usage: controller TARGET")?;
    let zygote = Zygote::builder(target).spawn()?;
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
