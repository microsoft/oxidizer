// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime and compile-time dispatch across scalar and SIMD scanners.

use crate::list::{self, EmptyMembers, TokenListScan};
use crate::range::{self, RangeMasks, WINDOW};
use crate::{base64, scalar, uri};

#[cfg(any(test, not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64"))))]
mod portable;

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
pub(super) use portable::all_base64_alphabet;
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
use portable::simple_uri_tail;

/// The shortest input eligible for the shared byte-classification scanners.
///
/// On an AMD EPYC 7763, x86-64 Criterion and Callgrind measurements at 16,
/// 17, and 31 bytes favored SIMD for token, `token68`, field-value, and
/// interesting-byte scans. Their 16-byte instruction counts fell by 40-63%
/// and wall-clock times by 31-65%. Architectures not measured here use a
/// two-vector crossover.
pub(super) const SIMD_THRESHOLD: usize = if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
    16
} else {
    32
};

/// The shortest input the ASCII case-insensitive equality scanner vectorizes.
///
/// The same x86-64 measurements did not justify lowering this crossover:
/// SIMD saved 12 instructions at 16 and 17 bytes but cost 185 at 31 bytes.
/// Raising it to 48 saved work at 47 bytes but regressed 32- and 33-byte
/// cases, so 32 remains the best measured monotonic cutoff. `AArch64` uses 32
/// because no measurements support another threshold.
const EQUALITY_SIMD_THRESHOLD: usize = 32;

/// The shortest reference the URI subset scanner vectorizes.
///
/// Redirect targets are usually short, and the scanner covers the trailing
/// bytes with one overlapping block rather than a scalar loop, so a single
/// vector's worth of input is already enough to pay for the dispatch.
const URI_SIMD_THRESHOLD: usize = 16;

#[inline]
pub(super) fn is_token(bytes: &[u8]) -> bool {
    if
    // #[gamma::skip(cond.always_false, tag = "equivalent", reason = "Both the scalar and vector token implementations independently reject empty input")]
    bytes.is_empty() {
        return false;
    }
    if
    // #[gamma::skip(cond.always_false, cond.always_true, cond.negate, relational.lt_to_gt, relational.lt_to_le, tag = "equivalent", reason = "Both token backends handle short inputs and implement the same byte grammar; only dispatch cost changes")]
    bytes.len() < SIMD_THRESHOLD {
        return scalar::is_token(bytes);
    }
    finish_with(
        // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "Replacing the optional optimized answer with None invokes the scalar validator for the same bytes")]
        optimized_is_token(bytes),
        || scalar::is_token(bytes),
    )
}

#[inline]
fn finish_with<T>(optimized: Option<T>, fallback: impl FnOnce() -> T) -> T {
    optimized.unwrap_or_else(fallback)
}

#[inline]
pub(super) fn is_token68(bytes: &[u8]) -> bool {
    if
    // #[gamma::skip(cond.always_false, cond.always_true, cond.negate, relational.lt_to_gt, relational.lt_to_le, tag = "equivalent", reason = "Both token68 backends handle short inputs and implement the same data-and-padding grammar")]
    bytes.len() < SIMD_THRESHOLD {
        return scalar::is_token68(bytes);
    }
    finish_with(
        // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "None selects the scalar token68 validator for the same bytes")]
        optimized_is_token68(bytes),
        || scalar::is_token68(bytes),
    )
}

#[inline]
pub(super) fn is_field_value(bytes: &[u8]) -> bool {
    if
    // #[gamma::skip(cond.always_false, cond.always_true, cond.negate, relational.lt_to_gt, relational.lt_to_le, tag = "equivalent", reason = "Both field-value backends handle short inputs and reject the same control bytes")]
    bytes.len() < SIMD_THRESHOLD {
        return scalar::is_field_value(bytes);
    }
    finish_with(
        // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "None invokes the scalar validator for the same bytes")]
        optimized_is_field_value(bytes),
        || scalar::is_field_value(bytes),
    )
}

#[inline]
pub(super) fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    if
    // #[gamma::skip(cond.always_false, cond.always_true, cond.negate, relational.lt_to_gt, relational.lt_to_le, tag = "equivalent", reason = "After the equal-length check, both comparison backends handle short inputs and fold only ASCII letters")]
    left.len() < EQUALITY_SIMD_THRESHOLD {
        return scalar::eq_ignore_ascii_case(left, right);
    }
    finish_eq_ignore_ascii_case(
        left,
        right,
        // #[gamma::skip(call.replace_with_default, call_result.default, tag = "equivalent", reason = "None invokes the scalar case-insensitive comparison for the same equal-length slices")]
        optimized_eq_ignore_ascii_case(left, right),
    )
}

#[inline]
fn finish_eq_ignore_ascii_case(left: &[u8], right: &[u8], optimized: Option<bool>) -> bool {
    finish_with(optimized, || scalar::eq_ignore_ascii_case(left, right))
}

#[inline]
pub(super) fn find_interesting(bytes: &[u8]) -> Option<usize> {
    if
    // #[gamma::skip(cond.always_false, cond.always_true, cond.negate, relational.lt_to_gt, relational.lt_to_le, tag = "equivalent", reason = "Both search backends handle short inputs and return the first interesting byte position")]
    bytes.len() < SIMD_THRESHOLD {
        return scalar::find_interesting(bytes);
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection.
        unsafe { find_interesting_x86(bytes, sse2_available()) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection.
        unsafe { find_interesting_arm(bytes, neon_available()) }
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    {
        scalar::find_interesting(bytes)
    }
}

#[inline]
pub(super) fn find_either(bytes: &[u8], first: u8, second: u8) -> Option<usize> {
    if
    // #[gamma::skip(cond.always_false, cond.always_true, cond.negate, relational.lt_to_gt, relational.lt_to_le, tag = "equivalent", reason = "Both search backends handle short inputs and return the first matching byte position")]
    bytes.len() < SIMD_THRESHOLD {
        return scalar::find_either(bytes, first, second);
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection.
        unsafe { find_either_x86(bytes, first, second, sse2_available()) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection.
        unsafe { find_either_arm(bytes, first, second, neon_available()) }
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    scalar::find_either(bytes, first, second)
}

/// Runs the `AArch64` byte-pair search with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON.
#[cfg(target_arch = "aarch64")]
unsafe fn find_either_arm(bytes: &[u8], first: u8, second: u8, neon: bool) -> Option<usize> {
    if neon {
        // SAFETY: the caller guarantees NEON support when `neon` is true.
        return unsafe { crate::arm::find_either(bytes, first, second) };
    }
    scalar::find_either(bytes, first, second)
}

/// Runs the x86 byte-pair search with already-detected features.
///
/// # Safety
///
/// If `sse2` is true, the processor must support SSE2.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn find_either_x86(bytes: &[u8], first: u8, second: u8, sse2: bool) -> Option<usize> {
    #[cfg(test)]
    tests::record_features(false, false, sse2);
    if sse2 {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 support when `sse2` is true.
        return unsafe { crate::x86::find_either(bytes, first, second) };
    }
    scalar::find_either(bytes, first, second)
}

/// Runs `AArch64` interesting-byte dispatch with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON.
#[cfg(target_arch = "aarch64")]
unsafe fn find_interesting_arm(bytes: &[u8], neon: bool) -> Option<usize> {
    if neon {
        // SAFETY: the caller supplies the result of NEON feature detection.
        return unsafe { crate::arm::find_interesting(bytes) };
    }
    scalar::find_interesting(bytes)
}

/// Runs the x86 interesting-byte dispatch with already-detected features.
///
/// # Safety
///
/// If `sse2` is true, the processor must support SSE2.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn find_interesting_x86(bytes: &[u8], sse2: bool) -> Option<usize> {
    #[cfg(test)]
    tests::record_features(false, false, sse2);
    if sse2 {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 support when `sse2` is true.
        return unsafe { crate::x86::find_interesting(bytes) };
    }
    scalar::find_interesting(bytes)
}

#[inline]
#[cfg(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64"))]
pub(super) fn all_base64_alphabet(bytes: &[u8]) -> bool {
    if
    // #[gamma::skip(cond.always_true, relational.lt_to_le, tag = "equivalent", reason = "Using the complete scalar alphabet validator more often changes only dispatch cost")]
    bytes.len() < URI_SIMD_THRESHOLD {
        return base64::all_base64_alphabet(bytes);
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        // SAFETY: the feature flags come from runtime or compile-time detection,
        // and the length check satisfies both backends' trailing-block requirement.
        unsafe { all_base64_alphabet_x86(bytes, sse42_available(), sse2_available()) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection.
        unsafe { all_base64_alphabet_arm(bytes, neon_available()) }
    }
}

/// Runs `AArch64` base64 dispatch with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON.
#[cfg(target_arch = "aarch64")]
unsafe fn all_base64_alphabet_arm(bytes: &[u8], neon: bool) -> bool {
    if neon {
        // SAFETY: the caller supplies the result of NEON feature detection.
        return unsafe { crate::arm::all_base64_alphabet(bytes) };
    }
    base64::all_base64_alphabet(bytes)
}

/// Runs x86 base64 dispatch with already-detected features.
///
/// # Safety
///
/// Every true feature flag must name an instruction set the processor supports,
/// and `bytes` must hold at least one whole vector.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn all_base64_alphabet_x86(bytes: &[u8], packed_ranges: bool, baseline: bool) -> bool {
    #[cfg(test)]
    tests::record_features(packed_ranges, false, baseline);
    if packed_ranges {
        #[cfg(test)]
        tests::record_selection("sse42");
        // SAFETY: the caller guarantees SSE4.2 support and a whole input vector.
        return unsafe { crate::x86::all_base64_alphabet_sse42(bytes) };
    }
    if baseline {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 support and a whole input vector.
        return unsafe { crate::x86::all_base64_alphabet_sse2(bytes) };
    }
    base64::all_base64_alphabet(bytes)
}

/// The shortest field line the token-list scanner vectorizes.
///
/// The scanner folds whole blocks and leaves the trailing bytes to the scalar
/// state machine, so a line shorter than one vector would pay for dispatch and
/// then run the scalar scan anyway.
const LIST_SIMD_THRESHOLD: usize = WIDTH;

/// The number of bytes one accelerated block covers on every architecture.
const WIDTH: usize = 16;

/// The longest line a single fold covers, which is two accelerated blocks.
///
/// The grammar costs more to fold than the class masks cost to build, so a
/// line this short is classified as two overlapping vectors and folded once,
/// while longer lines run the block loop.
const LIST_SHORT_LIMIT: usize = 2 * WIDTH;

#[inline]
pub(super) fn scan_token_list(bytes: &[u8], empty: EmptyMembers) -> TokenListScan {
    if bytes.len() < LIST_SIMD_THRESHOLD {
        return list::scan_token_list(bytes, empty);
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        // SAFETY: the feature flags come from runtime or compile-time detection,
        // and the input length satisfies every selected scanner's precondition.
        unsafe { scan_token_list_x86(bytes, empty, sse42_available(), sse2_available()) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection,
        // and the input contains at least one whole vector.
        unsafe { scan_token_list_arm(bytes, empty, neon_available()) }
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    {
        list::scan_token_list(bytes, empty)
    }
}

/// Runs `AArch64` token-list dispatch with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON and `bytes` must hold at
/// least one whole vector.
#[cfg(target_arch = "aarch64")]
unsafe fn scan_token_list_arm(bytes: &[u8], empty: EmptyMembers, neon: bool) -> TokenListScan {
    if neon {
        if bytes.len() <= LIST_SHORT_LIMIT {
            if bytes.len() == WIDTH {
                // SAFETY: the caller establishes NEON support and the line is
                // exactly one whole vector.
                return unsafe { crate::arm::scan_short_token_list::<false>(bytes, empty) };
            }
            // SAFETY: the caller establishes NEON support and the length checks
            // bound the line to two whole vectors.
            return unsafe { crate::arm::scan_short_token_list::<true>(bytes, empty) };
        }
        // SAFETY: the caller establishes NEON support and the length check in
        // `scan_token_list` satisfies the trailing-block requirement.
        return unsafe { crate::arm::scan_token_list(bytes, empty) };
    }
    list::scan_token_list(bytes, empty)
}

/// Runs x86 token-list dispatch with already-detected features.
///
/// # Safety
///
/// Every true feature flag must name an instruction set the processor supports,
/// and `bytes` must hold at least one whole vector.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn scan_token_list_x86(bytes: &[u8], empty: EmptyMembers, packed_ranges: bool, baseline: bool) -> TokenListScan {
    #[cfg(test)]
    tests::record_features(packed_ranges, false, baseline);
    if packed_ranges {
        if bytes.len() <= LIST_SHORT_LIMIT {
            if bytes.len() == WIDTH {
                #[cfg(test)]
                tests::record_selection("sse42-one");
                // SAFETY: the caller guarantees SSE4.2 and exactly one vector.
                return unsafe {
                    crate::x86::scan_short_token_list_sse42(
                        bytes, empty,
                        // #[gamma::skip(literal.bool_flip, tag = "equivalent", reason = "At exactly WIDTH bytes both loaded windows coincide; two-window folding shifts away the entire duplicate mask and folds the same WIDTH lanes")]
                        false,
                    )
                };
            }
            #[cfg(test)]
            tests::record_selection("sse42-two");
            // SAFETY: the caller guarantees SSE4.2 and at most two vectors.
            return unsafe { crate::x86::scan_short_token_list_sse42(bytes, empty, true) };
        }
        #[cfg(test)]
        tests::record_selection("sse42-long");
        // SAFETY: the caller guarantees SSE4.2 and at least one vector.
        return unsafe { crate::x86::scan_token_list_sse42(bytes, empty) };
    }
    if baseline {
        if bytes.len() <= LIST_SHORT_LIMIT {
            if bytes.len() == WIDTH {
                #[cfg(test)]
                tests::record_selection("sse2-one");
                // SAFETY: the caller guarantees SSE2 and exactly one vector.
                return unsafe {
                    crate::x86::scan_short_token_list_sse2(
                        bytes, empty,
                        // #[gamma::skip(literal.bool_flip, tag = "equivalent", reason = "At exactly WIDTH bytes both loaded windows coincide; two-window folding shifts away the entire duplicate mask and folds the same WIDTH lanes")]
                        false,
                    )
                };
            }
            #[cfg(test)]
            tests::record_selection("sse2-two");
            // SAFETY: the caller guarantees SSE2 and at most two vectors.
            return unsafe { crate::x86::scan_short_token_list_sse2(bytes, empty, true) };
        }
        #[cfg(test)]
        tests::record_selection("sse2-long");
        // SAFETY: the caller guarantees SSE2 and at least one vector.
        return unsafe { crate::x86::scan_token_list_sse2(bytes, empty) };
    }
    list::scan_token_list(bytes, empty)
}

/// Scans a `bytes=` field line for a range set that needs no parsing.
#[inline]
pub(super) fn scan_byte_range_set(bytes: &[u8], start: usize) -> bool {
    let Some(len) = bytes.len().checked_sub(start) else {
        return false;
    };
    // A payload of nothing and a payload past one window are both out of the
    // subset, and one wrapping subtraction folds the pair into one comparison.
    if
    // #[gamma::skip(cond.always_false, relational.ge_to_gt, literal.int_increment, tag = "equivalent", reason = "accepts independently rejects zero and oversized payload lengths; subtracting two only additionally rejects a one-byte payload, which cannot contain a valid byte range")]
    len.wrapping_sub(1) >= WINDOW {
        return false;
    }
    let Some(window) = range::window_of(bytes) else {
        return short_byte_range_set(bytes, len);
    };
    range::accepts(window, &classify_range(window), len)
}

/// Scans a field line shorter than one window by padding it into one.
#[cold]
#[inline(never)]
fn short_byte_range_set(bytes: &[u8], len: usize) -> bool {
    let mut padded = [range::FILLER; WINDOW];
    if !range::fill_window(bytes, &mut padded) {
        return false;
    }
    range::accepts(&padded, &classify_range(&padded), len)
}

/// Classifies one range window with the widest available kernel.
#[inline]
fn classify_range(window: &[u8; WINDOW]) -> RangeMasks {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection.
        unsafe { classify_range_x86(window, sse2_available()) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection.
        unsafe { classify_range_arm(window, neon_available()) }
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    {
        RangeMasks::scalar(window)
    }
}

/// Runs `AArch64` range classification with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON.
#[cfg(target_arch = "aarch64")]
unsafe fn classify_range_arm(window: &[u8; WINDOW], neon: bool) -> RangeMasks {
    if neon {
        // SAFETY: the caller supplies the result of NEON feature detection.
        return unsafe { crate::arm::range_masks(window) };
    }
    RangeMasks::scalar(window)
}

/// Runs x86 range classification with already-detected features.
///
/// # Safety
///
/// If `sse2` is true, the processor must support SSE2.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn classify_range_x86(window: &[u8; WINDOW], sse2: bool) -> RangeMasks {
    #[cfg(test)]
    tests::record_features(false, false, sse2);
    if sse2 {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 support when `sse2` is true.
        return unsafe { crate::x86::range_masks(window) };
    }
    RangeMasks::scalar(window)
}

#[inline]
pub(super) fn is_simple_uri_path(bytes: &[u8]) -> bool {
    if !uri::has_origin_relative_prefix(bytes) {
        return false;
    }
    simple_uri_tail(bytes)
}

#[inline]
pub(super) fn is_simple_uri_reference(bytes: &[u8]) -> bool {
    if uri::has_origin_relative_prefix(bytes) {
        return simple_uri_tail(bytes);
    }
    match uri::simple_authority_end(bytes) {
        Some(end) => simple_uri_tail(&bytes[end..]),
        None => false,
    }
}

/// Classifies every byte of a path, query, and fragment run.
#[cfg(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64"))]
fn simple_uri_tail(bytes: &[u8]) -> bool {
    if bytes.len() < URI_SIMD_THRESHOLD {
        return uri::simple_uri_tail(bytes, 0);
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        // SAFETY: the feature flags come from runtime or compile-time detection,
        // and the length check satisfies every backend's trailing-block requirement.
        unsafe { is_simple_uri_path_x86(bytes, sse42_available(), ssse3_available(), sse2_available()) }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: the feature flag comes from runtime or compile-time detection,
        // and the input contains at least one whole vector.
        unsafe { is_simple_uri_path_arm(bytes, neon_available()) }
    }
}

/// Runs `AArch64` URI dispatch with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON and `bytes` must hold at
/// least one whole vector.
#[cfg(target_arch = "aarch64")]
unsafe fn is_simple_uri_path_arm(bytes: &[u8], neon: bool) -> bool {
    if neon {
        // SAFETY: the caller supplies the result of NEON feature detection.
        return unsafe { crate::arm::is_simple_uri_tail(bytes) };
    }
    uri::simple_uri_tail(bytes, 0)
}

/// Runs x86 URI dispatch with already-detected features.
///
/// # Safety
///
/// Every true feature flag must name an instruction set the processor supports,
/// and `bytes` must hold at least one whole vector.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn is_simple_uri_path_x86(bytes: &[u8], packed_ranges: bool, shuffle_table: bool, baseline: bool) -> bool {
    #[cfg(test)]
    tests::record_features(packed_ranges, shuffle_table, baseline);
    if packed_ranges {
        #[cfg(test)]
        tests::record_selection("sse42");
        // SAFETY: the caller guarantees SSE4.2 and at least one vector.
        return unsafe { crate::x86::is_simple_uri_tail_sse42(bytes) };
    }
    if shuffle_table {
        #[cfg(test)]
        tests::record_selection("ssse3");
        // SAFETY: the caller guarantees SSSE3 and at least one vector.
        return unsafe { crate::x86::is_simple_uri_tail_ssse3(bytes) };
    }
    if baseline {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 and at least one vector.
        return unsafe { crate::x86::is_simple_uri_tail_sse2(bytes) };
    }
    uri::simple_uri_tail(bytes, 0)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn optimized_is_token(bytes: &[u8]) -> Option<bool> {
    // SAFETY: the feature flag comes from runtime or compile-time detection.
    unsafe { optimized_is_token_x86(bytes, sse2_available()) }
}

/// Runs x86 token dispatch with already-detected features.
///
/// # Safety
///
/// If `sse2` is true, the processor must support SSE2.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn optimized_is_token_x86(bytes: &[u8], sse2: bool) -> Option<bool> {
    if sse2 {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 support when `sse2` is true.
        return Some(unsafe { crate::x86::is_token(bytes) });
    }
    None
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn optimized_is_token68(bytes: &[u8]) -> Option<bool> {
    // SAFETY: the feature flags come from runtime or compile-time detection.
    unsafe { optimized_is_token68_x86(bytes, sse42_available(), sse2_available()) }
}

/// Runs x86 `token68` dispatch with already-detected features.
///
/// # Safety
///
/// Every true feature flag must name an instruction set the processor supports.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn optimized_is_token68_x86(bytes: &[u8], packed_ranges: bool, baseline: bool) -> Option<bool> {
    #[cfg(test)]
    tests::record_features(packed_ranges, false, baseline);
    if packed_ranges {
        #[cfg(test)]
        tests::record_selection("sse42");
        // SAFETY: the caller guarantees SSE4.2 support when `sse42` is true.
        return Some(unsafe { crate::x86::is_token68_sse42(bytes) });
    }
    if baseline {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 support when `sse2` is true.
        return Some(unsafe { crate::x86::is_token68_sse2(bytes) });
    }
    None
}

#[cfg(target_arch = "aarch64")]
fn optimized_is_token68(bytes: &[u8]) -> Option<bool> {
    // SAFETY: the feature flag comes from runtime or compile-time detection.
    unsafe { optimized_is_token68_arm(bytes, neon_available()) }
}

/// Runs `AArch64` `token68` dispatch with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON.
#[cfg(target_arch = "aarch64")]
unsafe fn optimized_is_token68_arm(bytes: &[u8], neon: bool) -> Option<bool> {
    if neon {
        // SAFETY: the caller supplies the result of NEON feature detection.
        return Some(unsafe { crate::arm::is_token68(bytes) });
    }
    None
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
fn optimized_is_token68(_: &[u8]) -> Option<bool> {
    None
}

#[cfg(target_arch = "aarch64")]
fn optimized_is_token(bytes: &[u8]) -> Option<bool> {
    // SAFETY: the feature flag comes from runtime or compile-time detection.
    unsafe { optimized_is_token_arm(bytes, neon_available()) }
}

/// Runs `AArch64` token dispatch with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON.
#[cfg(target_arch = "aarch64")]
unsafe fn optimized_is_token_arm(bytes: &[u8], neon: bool) -> Option<bool> {
    if neon {
        // SAFETY: the caller supplies the result of NEON feature detection.
        return Some(unsafe { crate::arm::is_token(bytes) });
    }
    None
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
fn optimized_is_token(_: &[u8]) -> Option<bool> {
    None
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn optimized_is_field_value(bytes: &[u8]) -> Option<bool> {
    // SAFETY: the feature flag comes from runtime or compile-time detection.
    unsafe { optimized_is_field_value_x86(bytes, sse2_available()) }
}

/// Runs x86 field-value dispatch with already-detected features.
///
/// # Safety
///
/// If `sse2` is true, the processor must support SSE2.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn optimized_is_field_value_x86(bytes: &[u8], sse2: bool) -> Option<bool> {
    if sse2 {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 support when `sse2` is true.
        return Some(unsafe { crate::x86::is_field_value(bytes) });
    }
    None
}

#[cfg(target_arch = "aarch64")]
fn optimized_is_field_value(bytes: &[u8]) -> Option<bool> {
    // SAFETY: the feature flag comes from runtime or compile-time detection.
    unsafe { optimized_is_field_value_arm(bytes, neon_available()) }
}

/// Runs `AArch64` field-value dispatch with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON.
#[cfg(target_arch = "aarch64")]
unsafe fn optimized_is_field_value_arm(bytes: &[u8], neon: bool) -> Option<bool> {
    if neon {
        // SAFETY: the caller supplies the result of NEON feature detection.
        return Some(unsafe { crate::arm::is_field_value(bytes) });
    }
    None
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
fn optimized_is_field_value(_: &[u8]) -> Option<bool> {
    None
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn optimized_eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> Option<bool> {
    // SAFETY: the feature flag comes from runtime or compile-time detection,
    // and the public dispatcher establishes equal lengths.
    unsafe { optimized_eq_ignore_ascii_case_x86(left, right, sse2_available()) }
}

/// Runs x86 ASCII-case dispatch with already-detected features.
///
/// # Safety
///
/// If `sse2` is true, the processor must support SSE2. `left` and `right` must
/// have equal lengths.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
unsafe fn optimized_eq_ignore_ascii_case_x86(left: &[u8], right: &[u8], sse2: bool) -> Option<bool> {
    if sse2 {
        #[cfg(test)]
        tests::record_selection("sse2");
        // SAFETY: the caller guarantees SSE2 support when `sse2` is true and
        // equal-length slices.
        return Some(unsafe { crate::x86::eq_ignore_ascii_case(left, right) });
    }
    None
}
#[cfg(target_arch = "aarch64")]
fn optimized_eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> Option<bool> {
    // SAFETY: the feature flag comes from runtime or compile-time detection,
    // and the public dispatcher establishes equal lengths.
    unsafe { optimized_eq_ignore_ascii_case_arm(left, right, neon_available()) }
}

/// Runs `AArch64` ASCII-case dispatch with already-detected features.
///
/// # Safety
///
/// If `neon` is true, the processor must support NEON. `left` and `right` must
/// have equal lengths.
#[cfg(target_arch = "aarch64")]
unsafe fn optimized_eq_ignore_ascii_case_arm(left: &[u8], right: &[u8], neon: bool) -> Option<bool> {
    if neon {
        // SAFETY: the caller supplies the result of NEON feature detection.
        return Some(unsafe { crate::arm::eq_ignore_ascii_case(left, right) });
    }
    None
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
fn optimized_eq_ignore_ascii_case(_: &[u8], _: &[u8]) -> Option<bool> {
    None
}

#[cfg(target_arch = "x86_64")]
const fn sse2_available() -> bool {
    true
}

#[cfg(target_arch = "x86")]
fn sse2_available() -> bool {
    #[cfg(feature = "std")]
    {
        use std::arch;

        arch::is_x86_feature_detected!("sse2")
    }
    #[cfg(not(feature = "std"))]
    {
        cfg!(target_feature = "sse2")
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn ssse3_available() -> bool {
    #[cfg(test)]
    if let Some((shuffle_table, _packed_ranges)) = tests::FEATURE_OVERRIDE.get() {
        return shuffle_table;
    }
    #[cfg(target_feature = "ssse3")]
    {
        true
    }
    #[cfg(all(not(target_feature = "ssse3"), feature = "std"))]
    {
        use std::arch;

        arch::is_x86_feature_detected!("ssse3")
    }
    #[cfg(all(not(target_feature = "ssse3"), not(feature = "std")))]
    {
        cached_ssse3_available()
    }
}

/// Caches SSSE3 detection where `std` feature detection is unavailable.
#[cfg(all(
    any(target_arch = "x86", target_arch = "x86_64"),
    not(feature = "std"),
    not(target_feature = "ssse3")
))]
fn cached_ssse3_available() -> bool {
    use core::sync::atomic::{AtomicU8, Ordering};

    static AVAILABLE: AtomicU8 = AtomicU8::new(0);
    let cached = AVAILABLE.load(Ordering::Relaxed);
    if cached != 0 {
        return cached == 2;
    }
    #[cfg(target_arch = "x86")]
    let features = core::arch::x86::__cpuid(1);
    #[cfg(target_arch = "x86_64")]
    let features = core::arch::x86_64::__cpuid(1);
    const SSSE3_BIT: u32 = 1 << 9;
    let available = features.ecx & SSSE3_BIT != 0;
    AVAILABLE.store(if available { 2 } else { 1 }, Ordering::Relaxed);
    available
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn sse42_available() -> bool {
    #[cfg(test)]
    if let Some((_shuffle_table, packed_ranges)) = tests::FEATURE_OVERRIDE.get() {
        return packed_ranges;
    }
    #[cfg(target_feature = "sse4.2")]
    {
        true
    }
    #[cfg(all(not(target_feature = "sse4.2"), feature = "std"))]
    {
        use std::arch;

        arch::is_x86_feature_detected!("sse4.2")
    }
    #[cfg(all(not(target_feature = "sse4.2"), not(feature = "std")))]
    {
        cached_sse42_available()
    }
}

#[cfg(all(
    any(target_arch = "x86", target_arch = "x86_64"),
    not(feature = "std"),
    not(target_feature = "sse4.2")
))]
fn cached_sse42_available() -> bool {
    use core::sync::atomic::{AtomicU8, Ordering};

    static AVAILABLE: AtomicU8 = AtomicU8::new(0);
    let cached = AVAILABLE.load(Ordering::Relaxed);
    if cached != 0 {
        return cached == 2;
    }
    #[cfg(target_arch = "x86")]
    let features = core::arch::x86::__cpuid(1);
    #[cfg(target_arch = "x86_64")]
    let features = core::arch::x86_64::__cpuid(1);
    const SSE42_BIT: u32 = 1 << 20;
    let available = features.ecx & SSE42_BIT != 0;
    AVAILABLE.store(if available { 2 } else { 1 }, Ordering::Relaxed);
    available
}

#[cfg(target_arch = "aarch64")]
fn neon_available() -> bool {
    #[cfg(feature = "std")]
    {
        use std::arch;

        arch::is_aarch64_feature_detected!("neon")
    }
    #[cfg(not(feature = "std"))]
    {
        cfg!(target_feature = "neon")
    }
}

#[cfg(any(feature = "benchmarking", feature = "test-util"))]
pub(super) fn backend() -> crate::Backend {
    backend_for(
        SIMD_THRESHOLD,
    )
}

#[cfg(any(feature = "benchmarking", feature = "test-util"))]
pub(super) fn backend_for(len: usize) -> crate::Backend {
    if len < SIMD_THRESHOLD {
        return crate::Backend::Scalar;
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        backend_for_x86(sse42_available(), sse2_available())
    }
    #[cfg(target_arch = "aarch64")]
    {
        backend_for_arm(neon_available())
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
    {
        crate::Backend::Scalar
    }
}

#[cfg(all(target_arch = "aarch64", any(feature = "benchmarking", feature = "test-util")))]
const fn backend_for_arm(neon: bool) -> crate::Backend {
    if neon { crate::Backend::Neon } else { crate::Backend::Scalar }
}

#[cfg(all(
    any(target_arch = "x86", target_arch = "x86_64"),
    any(feature = "benchmarking", feature = "test-util")
))]
fn backend_for_x86(packed_ranges: bool, baseline: bool) -> crate::Backend {
    #[cfg(test)]
    tests::record_features(packed_ranges, false, baseline);
    if packed_ranges {
        return crate::Backend::Sse42;
    }
    if baseline {
        return crate::Backend::Sse2;
    }
    crate::Backend::Scalar
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    std::thread_local! {
        pub(super) static FEATURE_OVERRIDE: core::cell::Cell<Option<(bool, bool)>> = const { core::cell::Cell::new(None) };
        static SELECTED: core::cell::Cell<Option<&'static str>> = const { core::cell::Cell::new(None) };
        static FEATURES: core::cell::Cell<(bool, bool, bool)> = const { core::cell::Cell::new((false, false, false)) };
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn gamma_afternoon_feature_queries_preserve_both_available_and_unavailable_detector_answers() {
        FEATURE_OVERRIDE.set(Some((false, false)));
        let unavailable = (super::ssse3_available(), super::sse42_available());
        FEATURE_OVERRIDE.set(Some((true, true)));
        let available = (super::ssse3_available(), super::sse42_available());
        FEATURE_OVERRIDE.set(None);
        assert_eq!(unavailable, (false, false));
        assert_eq!(available, (true, true));
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    pub(super) fn record_selection(kernel: &'static str) {
        SELECTED.set(Some(kernel));
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    pub(super) fn record_features(packed_ranges: bool, shuffle_table: bool, baseline: bool) {
        FEATURES.set((packed_ranges, shuffle_table, baseline));
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn selected<T>(expected: Option<&'static str>, invoke: impl FnOnce() -> T) -> T {
        SELECTED.set(None);
        let value = invoke();
        assert_eq!(SELECTED.get(), expected);
        value
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    #[expect(
        clippy::similar_names,
        reason = "SSE2, SSSE3, and SSE4.2 are the distinct instruction-set names under test"
    )]
    fn gamma_afternoon_dispatch_flags_choose_the_requested_kernel_and_preserve_scalar_results() {
        use crate::list::EmptyMembers;

        macro_rules! kernel {
            ($call:expr) => {{
                // SAFETY: flags are false or backed by host detection; every input has at least
                // one vector and equality inputs have identical lengths.
                unsafe { $call }
            }};
        }

        let actual_sse2 = std::is_x86_feature_detected!("sse2");
        let actual_sse42 = std::is_x86_feature_detected!("sse4.2");
        let actual_ssse3 = std::is_x86_feature_detected!("ssse3");
        for length in [16, 17, 24, 31, 32, 33, 47, 48, 64, 65] {
            for byte in [b'a', b';', b'=', b'#', b',', b'\n', 0x80] {
                let mut bytes = std::vec![b'a'; length];
                bytes[length / 2] = byte;
                for sse2 in [false, actual_sse2] {
                    let baseline = sse2.then_some("sse2");
                    {
                        assert_eq!(
                            selected(baseline, || kernel!(super::find_either_x86(&bytes, b';', b'=', sse2))),
                            crate::scalar::find_either(&bytes, b';', b'=')
                        );
                        assert_eq!(
                            selected(baseline, || kernel!(super::find_interesting_x86(&bytes, sse2))),
                            crate::scalar::find_interesting(&bytes)
                        );
                        assert_eq!(
                            selected(baseline, || kernel!(super::optimized_is_token_x86(&bytes, sse2))),
                            sse2.then(|| crate::scalar::is_token(&bytes))
                        );
                        assert_eq!(
                            selected(baseline, || kernel!(super::optimized_is_field_value_x86(&bytes, sse2))),
                            sse2.then(|| crate::scalar::is_field_value(&bytes))
                        );
                        assert_eq!(
                            selected(baseline, || kernel!(super::optimized_eq_ignore_ascii_case_x86(
                                &bytes, &bytes, sse2
                            ))),
                            sse2.then_some(true)
                        );
                        for sse42 in [false, actual_sse42] {
                            let kernel = if sse42 { Some("sse42") } else { baseline };
                            assert_eq!(
                                selected(kernel, || kernel!(super::all_base64_alphabet_x86(&bytes, sse42, sse2))),
                                crate::base64::all_base64_alphabet(&bytes)
                            );
                            assert_eq!(
                                selected(kernel, || kernel!(super::optimized_is_token68_x86(&bytes, sse42, sse2))),
                                (sse42 || sse2).then(|| crate::scalar::is_token68(&bytes))
                            );
                            let list_kernel = match (sse42, sse2, length) {
                                (true, _, 16) => Some("sse42-one"),
                                (true, _, 17..=32) => Some("sse42-two"),
                                (true, _, _) => Some("sse42-long"),
                                (false, true, 16) => Some("sse2-one"),
                                (false, true, 17..=32) => Some("sse2-two"),
                                (false, true, _) => Some("sse2-long"),
                                _ => None,
                            };
                            for empty in [EmptyMembers::Skip, EmptyMembers::Reject] {
                                assert_eq!(
                                    selected(list_kernel, || kernel!(super::scan_token_list_x86(&bytes, empty, sse42, sse2))),
                                    crate::list::scan_token_list(&bytes, empty)
                                );
                            }
                            for ssse3 in [false, actual_ssse3] {
                                let uri_kernel = if sse42 {
                                    Some("sse42")
                                } else if ssse3 {
                                    Some("ssse3")
                                } else {
                                    baseline
                                };
                                assert_eq!(
                                    selected(uri_kernel, || kernel!(super::is_simple_uri_path_x86(&bytes, sse42, ssse3, sse2))),
                                    crate::uri::simple_uri_tail(&bytes, 0)
                                );
                            }
                        }
                    }
                }
            }
        }
        for sse2 in [false, actual_sse2] {
            let window = [b'1'; super::WINDOW];
            assert_eq!(
                selected(sse2.then_some("sse2"), || kernel!(super::classify_range_x86(&window, sse2))),
                crate::range::RangeMasks::scalar(&window)
            );
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    #[expect(
        clippy::similar_names,
        reason = "SSE2, SSSE3, and SSE4.2 are the distinct instruction-set names under test"
    )]
    fn gamma_afternoon_detected_features_reach_the_selected_kernels_without_losing_the_answer() {
        let bytes = b"abcdefghijklmnopqrstuvwxyzABCDEF";
        let sse2 = std::is_x86_feature_detected!("sse2");
        let sse42 = std::is_x86_feature_detected!("sse4.2");
        let ssse3 = std::is_x86_feature_detected!("ssse3");
        let baseline = sse2.then_some("sse2");
        let packed = if sse42 { Some("sse42") } else { baseline };
        assert_eq!(selected(baseline, || super::find_either(bytes, b'A', b'z')), Some(25));
        assert_eq!(FEATURES.get(), (false, false, sse2));
        #[cfg(any(feature = "benchmarking", feature = "test-util"))]
        {
            let expected = if sse42 {
                crate::Backend::Sse42
            } else if sse2 {
                crate::Backend::Sse2
            } else {
                crate::Backend::Scalar
            };
            assert_eq!(super::backend_for(bytes.len()), expected);
            assert_eq!(FEATURES.get(), (sse42, false, sse2));
            for packed_ranges in [false, true] {
                for baseline in [false, true] {
                    let expected = if packed_ranges {
                        crate::Backend::Sse42
                    } else if baseline {
                        crate::Backend::Sse2
                    } else {
                        crate::Backend::Scalar
                    };
                    assert_eq!(super::backend_for_x86(packed_ranges, baseline), expected);
                }
            }
        }
        assert_eq!(selected(baseline, || super::find_interesting(bytes)), None);
        assert_eq!(FEATURES.get(), (false, false, sse2));
        assert_eq!(selected(baseline, || super::optimized_is_token(bytes)), sse2.then_some(true));
        assert_eq!(selected(baseline, || super::optimized_is_field_value(bytes)), sse2.then_some(true));
        assert_eq!(
            selected(baseline, || super::optimized_eq_ignore_ascii_case(bytes, bytes)),
            sse2.then_some(true)
        );
        assert_eq!(
            selected(packed, || super::optimized_is_token68(bytes)),
            (sse42 || sse2).then_some(true)
        );
        assert_eq!(FEATURES.get(), (sse42, false, sse2));
        assert!(selected(packed, || super::all_base64_alphabet(bytes)));
        assert_eq!(FEATURES.get(), (sse42, false, sse2));
        let uri_kernel = if sse42 {
            Some("sse42")
        } else if ssse3 {
            Some("ssse3")
        } else {
            baseline
        };
        assert!(selected(uri_kernel, || super::simple_uri_tail(bytes)));
        assert_eq!(FEATURES.get(), (sse42, ssse3, sse2));
        let list_kernel = if sse42 {
            Some("sse42-two")
        } else if sse2 {
            Some("sse2-two")
        } else {
            None
        };
        assert_eq!(
            selected(list_kernel, || super::scan_token_list(bytes, crate::list::EmptyMembers::Skip)),
            crate::list::TokenListScan::Members
        );
        assert_eq!(FEATURES.get(), (sse42, false, sse2));
        for length in [super::WIDTH - 1, super::WIDTH, super::WIDTH + 1] {
            let short = &bytes[..length];
            let vectorized = length >= super::WIDTH;
            assert!(selected(if vectorized { packed } else { None }, || super::all_base64_alphabet(
                short
            )));
            assert!(selected(if vectorized { uri_kernel } else { None }, || super::simple_uri_tail(
                short
            )));
            let expected = if !vectorized {
                None
            } else if sse42 {
                Some(if length == super::WIDTH { "sse42-one" } else { "sse42-two" })
            } else if sse2 {
                Some(if length == super::WIDTH { "sse2-one" } else { "sse2-two" })
            } else {
                None
            };
            assert_eq!(
                selected(expected, || super::scan_token_list(short, crate::list::EmptyMembers::Skip)),
                crate::list::TokenListScan::Members
            );
        }
        let window = [b'1'; super::WINDOW];
        assert_eq!(
            selected(baseline, || super::classify_range(&window)),
            crate::range::RangeMasks::scalar(&window)
        );
        assert_eq!(FEATURES.get(), (false, false, sse2));
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn gamma_round2_feature_queries_match_the_runtime_feature_bits() {
        assert_eq!(super::sse42_available(), std::is_x86_feature_detected!("sse4.2"));
        assert_eq!(super::ssse3_available(), std::is_x86_feature_detected!("ssse3"));
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn gamma_survivor_optimized_option_dispatch_and_lazy_fallback() {
        use core::cell::Cell;

        let calls = Cell::new(0);
        assert!(!super::finish_with(Some(false), || {
            calls.set(calls.get() + 1);
            true
        }));
        assert_eq!(calls.get(), 0);
        assert!(super::finish_with(None, || {
            calls.set(calls.get() + 1);
            true
        }));
        assert_eq!(calls.get(), 1);
        assert!(!super::finish_eq_ignore_ascii_case(b"same", b"same", Some(false)));
        let baseline = arch::is_x86_feature_detected!("sse2");
        let packed_ranges = arch::is_x86_feature_detected!("sse4.2");
        for length in [16, 17, 31, 32, 33, 48, 65] {
            for invalid in [false, true] {
                let mut input = vec![b'a'; length];
                if invalid {
                    input[length - 1] = b'\n';
                }
                let expected = !invalid;
                // SAFETY: the false flag disables native instructions.
                assert_eq!(unsafe { super::optimized_is_token_x86(&input, false) }, None);
                // SAFETY: the false flag disables native instructions.
                assert_eq!(unsafe { super::optimized_is_field_value_x86(&input, false) }, None);
                // SAFETY: both false flags disable native instructions.
                assert_eq!(unsafe { super::optimized_is_token68_x86(&input, false, false) }, None);
                // SAFETY: the false flag disables native instructions and slice lengths match.
                assert_eq!(unsafe { super::optimized_eq_ignore_ascii_case_x86(&input, &input, false) }, None);
                if baseline {
                    // SAFETY: SSE2 was detected and the input contains a full vector.
                    assert_eq!(unsafe { super::optimized_is_token_x86(&input, true) }, Some(expected));
                    // SAFETY: SSE2 was detected and the input contains a full vector.
                    assert_eq!(unsafe { super::optimized_is_field_value_x86(&input, true) }, Some(expected));
                    // SAFETY: SSE2 was detected and the input contains a full vector.
                    assert_eq!(unsafe { super::optimized_is_token68_x86(&input, false, true) }, Some(expected));
                    assert_eq!(
                        // SAFETY: SSE2 was detected and both slices have the same length.
                        unsafe { super::optimized_eq_ignore_ascii_case_x86(&input, &input, true) },
                        Some(true)
                    );
                    let other = vec![b'z'; length];
                    assert_eq!(
                        // SAFETY: SSE2 was detected and both slices have the same length.
                        unsafe { super::optimized_eq_ignore_ascii_case_x86(&input, &other, true) },
                        Some(false)
                    );
                    assert_eq!(super::optimized_is_token(&input), Some(expected));
                    assert_eq!(super::optimized_is_field_value(&input), Some(expected));
                    assert_eq!(super::optimized_is_token68(&input), Some(expected));
                    assert_eq!(super::optimized_eq_ignore_ascii_case(&input, &input), Some(true));
                    assert_eq!(
                        // SAFETY: SSE2 was detected and the input contains a full vector.
                        unsafe { super::scan_token_list_x86(&input, EmptyMembers::Reject, false, true) },
                        if invalid { TokenListScan::Rejected } else { TokenListScan::Members }
                    );
                }
                if packed_ranges {
                    // SAFETY: SSE4.2 was detected and the input contains a full vector.
                    assert_eq!(unsafe { super::optimized_is_token68_x86(&input, true, false) }, Some(expected));
                }
            }
            // SAFETY: the false flag selects scalar code.
            assert_eq!(unsafe { super::find_either_x86(b"aaaaaz", b'x', b'z', false) }, Some(5));
            // SAFETY: every feature flag is false, selecting scalar code.
            assert!(unsafe { super::is_simple_uri_path_x86(b"/fragment#one", false, false, false) });
            // SAFETY: every feature flag is false, selecting scalar code.
            assert!(!unsafe { super::is_simple_uri_path_x86(b"/fragment#one#two", false, false, false) });
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    use std::arch;
    #[cfg(not(feature = "std"))]
    use std::vec;

    use crate::list::{EmptyMembers, TokenListScan};

    #[test]
    fn dispatch_covers_scalar_and_vector_length_boundaries() {
        assert!(!super::is_token(b""));
        assert!(super::is_token(b"short-token"));
        assert!(!super::is_token(b"short token"));

        let valid = vec![b'a'; super::SIMD_THRESHOLD + 17];
        assert!(super::is_token(&valid));
        assert!(super::is_token68(&valid));
        assert!(super::is_field_value(&valid));
        assert!(super::eq_ignore_ascii_case(&valid, &valid));
        assert_eq!(super::find_interesting(&valid), None);
        assert!(super::all_base64_alphabet(&valid));

        let mut interesting = valid;
        interesting[super::SIMD_THRESHOLD + 3] = b';';
        assert_eq!(super::find_interesting(&interesting), Some(super::SIMD_THRESHOLD + 3));
    }

    #[test]
    fn tuned_scanners_match_scalar_around_both_crossovers() {
        for length in [15, 16, 17, 31, 32, 33, 47, 48] {
            let valid = vec![b'a'; length];
            let folded = vec![b'A'; length];

            assert_eq!(super::is_token(&valid), crate::scalar::is_token(&valid));
            assert_eq!(super::is_token68(&valid), crate::scalar::is_token68(&valid));
            assert_eq!(super::is_field_value(&valid), crate::scalar::is_field_value(&valid));
            assert_eq!(
                super::eq_ignore_ascii_case(&valid, &folded),
                crate::scalar::eq_ignore_ascii_case(&valid, &folded)
            );
            assert_eq!(super::find_interesting(&valid), crate::scalar::find_interesting(&valid));

            let mut rejected = valid.clone();
            rejected[length - 1] = b' ';
            assert_eq!(super::is_token(&rejected), crate::scalar::is_token(&rejected));
            assert_eq!(super::is_token68(&rejected), crate::scalar::is_token68(&rejected));
            assert_eq!(super::find_interesting(&rejected), crate::scalar::find_interesting(&rejected));

            rejected[length - 1] = b'\n';
            assert_eq!(super::is_field_value(&rejected), crate::scalar::is_field_value(&rejected));

            let mut unequal = folded;
            unequal[length - 1] = b'b';
            assert_eq!(
                super::eq_ignore_ascii_case(&valid, &unequal),
                crate::scalar::eq_ignore_ascii_case(&valid, &unequal)
            );
        }
    }

    #[test]
    fn dispatch_rejects_invalid_range_offsets_and_scans_list_shapes() {
        assert!(!super::scan_byte_range_set(b"bytes=0-1", 64));
        assert!(!super::scan_byte_range_set(b"bytes=", 6));
        assert!(!super::scan_byte_range_set(b"1-2", 0));
        assert!(super::scan_byte_range_set(b"bytes=1-2", 6));
        assert!(super::scan_byte_range_set(b"bytes=1234-5678", 6));

        for length in [16, 17, 32, 33, 48] {
            let input = vec![b'a'; length];
            assert_eq!(super::scan_token_list(&input, EmptyMembers::Skip), TokenListScan::Members);
        }
        let mut rejected = vec![b'a'; 48];
        rejected[31] = b' ';
        assert_eq!(super::scan_token_list(&rejected, EmptyMembers::Skip), TokenListScan::Rejected);
    }

    #[test]
    fn uri_dispatch_checks_prefix_short_tail_and_vector_tail() {
        assert!(!super::is_simple_uri_path(b"relative"));
        assert!(super::is_simple_uri_path(b"/short#tail"));

        let mut path = vec![b'a'; 49];
        path[0] = b'/';
        assert!(super::is_simple_uri_path(&path));
        path[40] = b'%';
        assert!(!super::is_simple_uri_path(&path));
    }

    #[cfg(any(feature = "benchmarking", feature = "test-util"))]
    #[test]
    fn backend_selection_respects_the_scalar_threshold() {
        assert_eq!(super::backend_for(super::SIMD_THRESHOLD - 1), crate::Backend::Scalar);
        assert_eq!(super::backend(), super::backend_for(super::SIMD_THRESHOLD));
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn x86_feature_detection_matches_compile_time_or_runtime_support() {
        #[cfg(target_arch = "x86_64")]
        assert!(super::sse2_available());
        #[cfg(target_arch = "x86")]
        assert_eq!(
            super::sse2_available(),
            cfg!(target_feature = "sse2") || cfg!(feature = "std") && arch::is_x86_feature_detected!("sse2")
        );
        assert_eq!(
            super::ssse3_available(),
            cfg!(target_feature = "ssse3") || arch::is_x86_feature_detected!("ssse3")
        );
        assert_eq!(
            super::sse42_available(),
            cfg!(target_feature = "sse4.2") || arch::is_x86_feature_detected!("sse4.2")
        );
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn lower_x86_dispatch_tiers_and_scalar_fallbacks_are_directly_tested() {
        #[cfg(target_arch = "x86")]
        if !arch::is_x86_feature_detected!("sse2") {
            return;
        }

        let long = [b'a'; 48];
        assert!(super::finish_with(None, || true));
        assert!(!super::finish_with(Some(false), || true));
        assert!(super::finish_eq_ignore_ascii_case(&long, &long, None));

        let mut interesting = long;
        interesting[37] = b',';
        assert_eq!(
            // SAFETY: x86-64 guarantees SSE2; the x86 guard above detects it.
            unsafe { super::find_interesting_x86(&interesting, true) },
            Some(37)
        );
        assert_eq!(
            // SAFETY: no feature instruction is used when the flag is false.
            unsafe { super::find_interesting_x86(&interesting, false) },
            Some(37)
        );
        assert_eq!(
            // SAFETY: x86-64 guarantees SSE2; the x86 guard above detects it.
            unsafe { super::find_either_x86(&interesting, b',', b'"', true) },
            Some(37)
        );
        assert_eq!(
            // SAFETY: no feature instruction is used when the flag is false.
            unsafe { super::find_either_x86(&interesting, b',', b'"', false) },
            Some(37)
        );

        assert!(
            // SAFETY: x86-64 guarantees SSE2; the x86 guard above detects it,
            // and the input holds at least one whole vector.
            unsafe { super::all_base64_alphabet_x86(&long, false, true) }
        );
        assert!(
            // SAFETY: false feature flags select the scalar implementation.
            unsafe { super::all_base64_alphabet_x86(&long, false, false) }
        );

        for input in [&[b'a'; 16][..], &[b'a'; 17][..], &[b'a'; 33][..]] {
            assert_eq!(
                // SAFETY: x86-64 guarantees SSE2; the x86 guard above detects it,
                // and every input holds at least one whole vector.
                unsafe { super::scan_token_list_x86(input, EmptyMembers::Skip, false, true) },
                TokenListScan::Members
            );
            assert_eq!(
                // SAFETY: false feature flags select the scalar implementation.
                unsafe { super::scan_token_list_x86(input, EmptyMembers::Skip, false, false) },
                TokenListScan::Members
            );
        }

        let window = *b"0123456789-, \txy";
        assert_eq!(
            // SAFETY: x86-64 guarantees SSE2; the x86 guard above detects it.
            unsafe { super::classify_range_x86(&window, true) },
            crate::range::RangeMasks::scalar(&window)
        );
        assert_eq!(
            // SAFETY: a false feature flag selects scalar classification.
            unsafe { super::classify_range_x86(&window, false) },
            crate::range::RangeMasks::scalar(&window)
        );

        assert!(
            // SAFETY: x86-64 guarantees SSE2; the x86 guard above detects it,
            // and the input holds at least one whole vector.
            unsafe { super::is_simple_uri_path_x86(&long, false, false, true) }
        );
        assert!(
            // SAFETY: false feature flags select the scalar implementation.
            unsafe { super::is_simple_uri_path_x86(&long, false, false, false) }
        );
        let ssse3 = arch::is_x86_feature_detected!("ssse3");
        assert_eq!(
            ssse3.then(|| {
                // SAFETY: runtime detection establishes SSSE3 support and the input
                // holds at least one whole vector.
                unsafe { super::is_simple_uri_path_x86(&long, false, true, true) }
            }),
            ssse3.then_some(true)
        );

        assert_eq!(
            // SAFETY: a false feature flag selects no optimized token backend.
            unsafe { super::optimized_is_token_x86(&long, false) },
            None
        );
        assert_eq!(
            // SAFETY: x86-64 guarantees SSE2; the x86 guard above detects it.
            unsafe { super::optimized_is_token68_x86(&long, false, true) },
            Some(true)
        );
        assert_eq!(
            // SAFETY: false feature flags select no optimized `token68` backend.
            unsafe { super::optimized_is_token68_x86(&long, false, false) },
            None
        );
        assert_eq!(
            // SAFETY: a false feature flag selects no optimized field-value backend.
            unsafe { super::optimized_is_field_value_x86(&long, false) },
            None
        );
        assert_eq!(
            // SAFETY: a false feature flag selects no optimized case-folding backend.
            unsafe { super::optimized_eq_ignore_ascii_case_x86(&long, &long, false) },
            None
        );

        #[cfg(any(feature = "benchmarking", feature = "test-util"))]
        {
            assert_eq!(super::backend_for_x86(false, true), crate::Backend::Sse2);
            assert_eq!(super::backend_for_x86(false, false), crate::Backend::Scalar);
        }
    }

    #[cfg(all(
        not(feature = "std"),
        any(target_arch = "x86", target_arch = "x86_64"),
        not(target_feature = "ssse3")
    ))]
    #[test]
    fn no_std_ssse3_detection_is_stable_after_caching() {
        let expected = arch::is_x86_feature_detected!("ssse3");
        assert_eq!(super::ssse3_available(), expected);
        for _ in 0..8 {
            assert_eq!(super::ssse3_available(), expected);
        }
    }

    #[cfg(all(
        not(feature = "std"),
        any(target_arch = "x86", target_arch = "x86_64"),
        not(target_feature = "sse4.2")
    ))]
    #[test]
    fn no_std_sse42_detection_is_stable_after_caching() {
        let expected = arch::is_x86_feature_detected!("sse4.2");
        assert_eq!(super::sse42_available(), expected);
        for _ in 0..8 {
            assert_eq!(super::sse42_available(), expected);
        }
    }

    #[cfg(all(not(feature = "std"), target_arch = "x86"))]
    #[test]
    fn no_std_sse2_detection_matches_target_features() {
        assert_eq!(super::sse2_available(), cfg!(target_feature = "sse2"));
    }

    #[cfg(all(not(feature = "std"), target_arch = "aarch64"))]
    #[test]
    fn no_std_neon_detection_matches_target_features() {
        assert_eq!(super::neon_available(), cfg!(target_feature = "neon"));
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn arm_scalar_fallbacks_are_directly_tested() {
        let long = [b'a'; 48];
        let mut interesting = long;
        interesting[37] = b',';
        // SAFETY: the false feature flag selects only the scalar fallback.
        assert_eq!(unsafe { super::find_interesting_arm(&interesting, false) }, Some(37));
        // SAFETY: the false feature flag selects only the scalar fallback.
        assert_eq!(unsafe { super::find_either_arm(&interesting, b',', b'"', false) }, Some(37));
        // SAFETY: the false feature flag selects only the scalar fallback.
        assert!(unsafe { super::all_base64_alphabet_arm(&long, false) });

        for input in [&[b'a'; 16][..], &[b'a'; 17][..], &[b'a'; 33][..]] {
            // SAFETY: the false feature flag selects only the scalar fallback,
            // and every input contains at least one whole vector.
            let scan = unsafe { super::scan_token_list_arm(input, EmptyMembers::Skip, false) };
            assert_eq!(scan, TokenListScan::Members);
        }

        let window = *b"0123456789-, \txy";
        // SAFETY: the false feature flag selects only the scalar fallback.
        let masks = unsafe { super::classify_range_arm(&window, false) };
        assert_eq!(masks, crate::range::RangeMasks::scalar(&window));
        // SAFETY: the false feature flag selects only the scalar fallback,
        // and the input contains at least one whole vector.
        assert!(unsafe { super::is_simple_uri_path_arm(&long, false) });
        // SAFETY: the false feature flag selects only the scalar fallback.
        assert_eq!(unsafe { super::optimized_is_token_arm(&long, false) }, None);
        // SAFETY: the false feature flag selects only the scalar fallback.
        assert_eq!(unsafe { super::optimized_is_token68_arm(&long, false) }, None);
        // SAFETY: the false feature flag selects only the scalar fallback.
        assert_eq!(unsafe { super::optimized_is_field_value_arm(&long, false) }, None);
        // SAFETY: the false feature flag selects only the scalar fallback, and
        // the slices have equal lengths.
        assert_eq!(unsafe { super::optimized_eq_ignore_ascii_case_arm(&long, &long, false) }, None);

        #[cfg(any(feature = "benchmarking", feature = "test-util"))]
        assert_eq!(super::backend_for_arm(false), crate::Backend::Scalar);
    }
}
