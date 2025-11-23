// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::{RedactedDisplay, RedactionEngine};
use once_cell::sync::OnceCell;

static REDACTION_ENGINE: OnceCell<RedactionEngine> = OnceCell::new();

#[expect(clippy::unwrap_used, reason = "Infallible after initialization")]
pub fn set_redaction_engine_for_logging(engine: RedactionEngine) {
    REDACTION_ENGINE.set(engine).unwrap();
}

#[expect(clippy::unwrap_used, reason = "Infallible after initialization")]
pub fn redacted_display(value: &impl RedactedDisplay) -> String {
    let mut output = String::new();
    _ = REDACTION_ENGINE.get().unwrap().redacted_display(value, &mut output);
    output
}

macro_rules! log {
    (@fmt ($name:ident) = $value:expr) => {
        format!("{}={}", stringify!($name), $value)
    };

    (@fmt ($name:ident):? = $value:expr) => {
        format!("{}={:?}", stringify!($name), $value)
    };

    (@fmt ($name:ident):@ = $value:expr) => {
        format!("{}={}", stringify!($name), crate::logging::redacted_display(&$value))
    };

    ($($name:ident $(: $kind:tt)? = $value:expr),* $(,)?) => {
        let mut parts: Vec<String> = Vec::new();
        $(
            parts.push(log!(@fmt ($name)$(: $kind)? = $value));
        )*
        println!("LOG RECORD: {}", parts.join(", "));
    };
}

pub(crate) use log;
