// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Round-two terminal-error label ownership.

#[test]
fn gamma_round2_terminal_error_retains_the_original_borrowed_label() {
    let error = std::io::Error::other("terminal");
    let mut calls = 0;
    let label = ohno::ErrorLabel::from_error_chain(&error, |_| {
        calls += 1;
        Some(ohno::ErrorLabel::from_static("terminal"))
    });
    assert_eq!(calls, 1);
    assert!(matches!(label.into_cow(), std::borrow::Cow::Borrowed("terminal")));
}
