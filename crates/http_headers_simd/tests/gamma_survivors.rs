// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Boundary and byte-class contracts for the public scanners.

use http_headers_simd::{
    EmptyMembers, TokenListScan, all_base64_alphabet, as_simple_uri_reference, ascii_str, eq_ignore_ascii_case, find_either,
    find_interesting, is_field_value, is_simple_uri_path, is_token, is_token68, scan_byte_range_set, scan_token_list,
};

#[test]
fn gamma_round2_portable_reference_branches_preserve_positive_and_negative_cases() {
    let alphabet = [b'A'; 64];
    assert!(all_base64_alphabet(&alphabet));
    assert!(!all_base64_alphabet(&[b'!'; 64]));
    let mut path = [b'a'; 64];
    path[0] = b'/';
    assert!(is_simple_uri_path(&path));
    path[63] = b' ';
    assert!(!is_simple_uri_path(&path));
}

#[cfg(feature = "benchmarking")]
#[test]
fn gamma_round2_neon_wrapper_reports_eligibility_and_validation() {
    use http_headers_simd::benchmarking;

    #[cfg(all(feature = "std", target_arch = "aarch64"))]
    let available = std::arch::is_aarch64_feature_detected!("neon");
    #[cfg(not(all(feature = "std", target_arch = "aarch64")))]
    let available = false;
    assert_eq!(
        benchmarking::is_simple_uri_path_neon(b"/abcdefghijklmnop"),
        available.then_some(true)
    );
    assert_eq!(
        benchmarking::is_simple_uri_path_neon(b"/abcdef ghijklmnop"),
        available.then_some(false)
    );
    assert_eq!(benchmarking::is_simple_uri_path_neon(b"/short"), None);
}

#[test]
fn gamma_round2_range_payload_occupies_every_mask_lane() {
    assert!(scan_byte_range_set(b"123456789012345-", 0));
    assert!(scan_byte_range_set(b"bytes=0-", 6));
    assert!(!scan_byte_range_set(b"bytes=0-", 7));
    #[cfg(feature = "benchmarking")]
    {
        assert!(http_headers_simd::benchmarking::scan_byte_range_set_scalar(b"123456789012345-", 0));
        assert!(http_headers_simd::benchmarking::scan_byte_range_set_scalar(b"bytes=0-", 6));
        assert!(!http_headers_simd::benchmarking::scan_byte_range_set_scalar(b"bytes=0-", 7));
    }
}

#[test]
fn gamma_round2_case_equal_prefix_is_not_equal_to_longer_input() {
    for len in [0, 1, 15, 16, 17, 31, 32, 33, 48, 64, 65] {
        let left = vec![b'A'; len];
        let right = vec![b'a'; len + 1];
        assert!(!eq_ignore_ascii_case(&left, &right), "{len}");
    }
}

#[test]
fn gamma_survivor_byte_classes_and_search_lanes() {
    for length in [1, 15, 16, 17, 31, 32, 33, 47, 48, 49, 63, 64, 65] {
        for lane in 0..length {
            for byte in 0..=u8::MAX {
                let mut input = vec![b'a'; length];
                input[lane] = byte;
                let token = byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
                let base64 = byte.is_ascii_alphanumeric() || b"+/".contains(&byte);
                let token68 = byte.is_ascii_alphanumeric() || b"-._~+/".contains(&byte) || byte == b'=' && lane > 0 && lane + 1 == length;
                assert_eq!(is_token(&input), token, "{length}/{lane}/{byte}");
                assert_eq!(is_token68(&input), token68, "{length}/{lane}/{byte}");
                assert_eq!(all_base64_alphabet(&input), base64, "{length}/{lane}/{byte}");
                assert_eq!(is_field_value(&input), byte == b'\t' || byte >= 32 && byte != 127);
                assert_eq!(ascii_str(&input).is_some(), byte < 128);
                assert_eq!(find_interesting(&input), b",;\"\\ \t".contains(&byte).then_some(lane));
                assert_eq!(find_either(&input, byte, b'z'), Some(if byte == b'a' { 0 } else { lane }));
                assert_eq!(find_either(&input, b'z', byte), Some(if byte == b'a' { 0 } else { lane }));
            }
        }
    }
    assert_eq!(ascii_str(b""), Some(""));
    assert!(!is_token(b""));
    assert!(!is_token68(b""));
    assert!(is_field_value(b""));
    assert!(all_base64_alphabet(b""));
    assert_eq!(find_either(b"", 0, 255), None);
    for length in [16, 17, 32, 33, 64, 65] {
        let mut input = vec![b'a'; length];
        input[1] = b'=';
        input[length - 1] = b'=';
        assert!(!is_token68(&input));
        input[1..].fill(b'=');
        assert!(is_token68(&input));
        input[0] = b'=';
        assert!(!is_token68(&input));
        assert_eq!(find_either(&input, b'x', b'y'), None);
    }
}

#[test]
fn gamma_survivor_case_fold_all_byte_pairs_and_tails() {
    for left in 0..=u8::MAX {
        for right in 0..=u8::MAX {
            assert_eq!(
                eq_ignore_ascii_case(&[left], &[right]),
                left.eq_ignore_ascii_case(&right),
                "{left}/{right}"
            );
        }
    }
    for length in [15, 16, 17, 31, 32, 33, 47, 48, 49, 64, 65] {
        for lane in 0..length {
            let left = vec![b'A'; length];
            let mut right = vec![b'a'; length];
            assert!(eq_ignore_ascii_case(&left, &right));
            for byte in [b'@', b'[', b'`', b'{', b'B', 0x80, 0xff] {
                right[lane] = byte;
                assert!(!eq_ignore_ascii_case(&left, &right));
            }
            assert!(!eq_ignore_ascii_case(&left[..length - 1], &right));
        }
    }
}

#[test]
fn gamma_survivor_uri_structure_and_lane_boundaries() {
    for length in [16, 17, 31, 32, 33, 47, 48, 49, 64, 65] {
        for lane in 1..length {
            for byte in 0..=u8::MAX {
                let mut path = vec![b'a'; length];
                path[0] = b'/';
                path[lane] = byte;
                let allowed = byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?#".contains(&byte);
                let expected = allowed && !(lane == 1 && byte == b'/');
                assert_eq!(is_simple_uri_path(&path), expected, "{length}/{lane}/{byte}");
                assert_eq!(as_simple_uri_reference(&path).is_some(), expected);
            }
        }
        let mut path = vec![b'a'; length];
        path[0] = b'/';
        path[1] = b'#';
        path[length - 1] = b'#';
        assert!(!is_simple_uri_path(&path));
    }
    for valid in [
        "/",
        "/a?b#c",
        "a://host",
        "A+1.2-3://host:123/path?query#fragment",
        "https://host:0",
        "https://host:",
        "https://host?query",
    ] {
        assert_eq!(as_simple_uri_reference(valid.as_bytes()), Some(valid));
    }
    for invalid in [
        "",
        "a",
        "//host",
        "1://host",
        "a:/host",
        "a://",
        "a://:1",
        "a://host:abc",
        "a://user@host",
        "a://[::1]",
        "/a%20b",
        "/a#b#c",
        "a://host#x#y",
    ] {
        assert_eq!(as_simple_uri_reference(invalid.as_bytes()), None, "{invalid}");
    }
}

#[test]
fn gamma_survivor_token_list_transitions_across_windows() {
    for length in [15, 16, 17, 31, 32, 33, 47, 48, 49, 64, 65] {
        for lane in 0..length {
            let mut input = vec![b'a'; length];
            for byte in [b',', b' ', b'\t', b'"', b';', b'\\', 0, 127, 255] {
                input[lane] = byte;
                for policy in [EmptyMembers::Skip, EmptyMembers::Reject] {
                    let expected = match byte {
                        b',' if lane == 0 || lane + 1 == length => {
                            if policy == EmptyMembers::Skip {
                                TokenListScan::Members
                            } else {
                                TokenListScan::Rejected
                            }
                        }
                        b',' => TokenListScan::Members,
                        b' ' | b'\t' if lane == 0 || lane + 1 == length => TokenListScan::Members,
                        _ => TokenListScan::Rejected,
                    };
                    assert_eq!(scan_token_list(&input, policy), expected, "{length}/{lane}/{byte}/{policy:?}");
                }
            }
        }
        let mut input = vec![b' '; length];
        input[0] = b'a';
        input[length - 1] = b'b';
        assert_eq!(scan_token_list(&input, EmptyMembers::Skip), TokenListScan::Rejected);
        input[1] = b',';
        assert_eq!(scan_token_list(&input, EmptyMembers::Skip), TokenListScan::Members);
        input[length - 1] = b',';
        assert_eq!(scan_token_list(&input, EmptyMembers::Reject), TokenListScan::Rejected);
    }
    assert_eq!(scan_token_list(b"", EmptyMembers::Skip), TokenListScan::Empty);
    assert_eq!(scan_token_list(b", \t,", EmptyMembers::Skip), TokenListScan::Empty);
}

#[test]
fn gamma_survivor_range_numeric_order_offsets_and_padding() {
    assert!(scan_byte_range_set(b"bytes=-123456789012345", 6));
    for first in [0, 1, 9, 10, 11, 19, 20, 99, 100, 101, 999, 1000] {
        for last in [0, 1, 9, 10, 11, 19, 20, 99, 100, 101, 999, 1000] {
            let text = format!("bytes={first}-{last}");
            assert_eq!(scan_byte_range_set(text.as_bytes(), 6), first <= last, "{text}");
        }
    }
    for payload in ["0-", "-0", "1-2,3-4", ",1-2,", "1-2, 3-", "1-2,\t-3", "123456-12345678"] {
        let text = format!("bytes={payload}");
        assert!(scan_byte_range_set(text.as_bytes(), 6), "{text}");
    }
    for payload in [
        "",
        "-",
        "1",
        "1--2",
        "--2",
        "01-2",
        "1-02",
        "-01",
        "1 -2",
        "1- 2",
        " 1-2",
        "1-2 ",
        ",",
        "1-2,  3-4",
        "12345678-12345678",
        "1-2x",
    ] {
        let text = format!("bytes={payload}");
        assert!(!scan_byte_range_set(text.as_bytes(), 6), "{text}");
    }
    for offset in [9, 10, usize::MAX] {
        assert!(!scan_byte_range_set(b"bytes=1-2", offset));
    }
}

#[cfg(feature = "benchmarking")]
#[test]
fn gamma_survivor_benchmark_wrappers_report_eligibility() {
    use http_headers_simd::benchmarking;
    assert!(benchmarking::scan_byte_range_set_scalar(b"bytes=1-2", 6));
    assert_eq!(benchmarking::is_simple_uri_path_sse2(b"relativeaaaaaaaa"), None);
    for length in [15, 16, 17, 32, 33] {
        let mut input = vec![b'a'; length];
        input[0] = b'/';
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            assert_eq!(
                benchmarking::is_simple_uri_path_sse2(&input),
                (length >= 16 && std::is_x86_feature_detected!("sse2")).then_some(true)
            );
            assert_eq!(
                benchmarking::is_simple_uri_path_ssse3(&input),
                (length >= 16 && std::is_x86_feature_detected!("ssse3")).then_some(true)
            );
            assert_eq!(
                benchmarking::is_simple_uri_path_sse42(&input),
                (length >= 16 && std::is_x86_feature_detected!("sse4.2")).then_some(true)
            );
        }
    }
}
