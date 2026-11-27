// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared formatting for monitor models, reports, and terminal views.

pub(super) fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

pub(super) fn format_bytes(bytes: u64) -> String {
    const KIBIBYTE: u64 = 1024;
    const MEBIBYTE: u64 = KIBIBYTE * 1024;
    const GIBIBYTE: u64 = MEBIBYTE * 1024;
    if bytes >= GIBIBYTE {
        format_decimal(bytes, GIBIBYTE, "GiB", true)
    } else if bytes >= MEBIBYTE {
        format_decimal(bytes, MEBIBYTE, "MiB", true)
    } else if bytes >= KIBIBYTE {
        format_decimal(bytes, KIBIBYTE, "KiB", true)
    } else {
        format!("{bytes} B")
    }
}

pub(super) fn format_runtime_duration(nanos: u64) -> String {
    if nanos >= 1_000_000_000 {
        format_decimal(nanos, 1_000_000_000, "s", false)
    } else if nanos >= 1_000_000 {
        format_decimal(nanos, 1_000_000, "ms", false)
    } else if nanos >= 1_000 {
        format_decimal(nanos, 1_000, "us", false)
    } else {
        format!("{nanos}ns")
    }
}

pub(super) fn format_prose_duration(nanos: u64) -> String {
    if nanos >= 1_000_000_000 {
        format_decimal(nanos, 1_000_000_000, "s", true)
    } else if nanos >= 1_000_000 {
        format_decimal(nanos, 1_000_000, "ms", true)
    } else if nanos >= 1_000 {
        format_decimal(nanos, 1_000, "us", true)
    } else {
        format!("{nanos}ns")
    }
}

pub(super) fn format_histogram_duration(nanos: u64) -> String {
    match nanos {
        0..1_000 => format!("{nanos}ns"),
        1_000..1_000_000 => format!("{}us", nanos / 1_000),
        1_000_000..1_000_000_000 => format!("{}ms", nanos / 1_000_000),
        1_000_000_000..10_000_000_000 => {
            format!("{}.{:01}s", nanos / 1_000_000_000, nanos % 1_000_000_000 / 100_000_000)
        }
        _ => format!("{}s", nanos / 1_000_000_000),
    }
}

fn format_decimal(value: u64, unit: u64, suffix: &str, space: bool) -> String {
    let whole = value / unit;
    let hundredths = value % unit * 100 / unit;
    let separator = if space { " " } else { "" };
    format!("{whole}.{hundredths:02}{separator}{suffix}")
}
