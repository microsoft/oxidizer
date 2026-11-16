// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Independent byte oracles across scalar and vector tail boundaries.

use http_headers_simd::{eq_ignore_ascii_case, find_either, find_interesting, is_field_value, is_token, is_token68};

#[test]
fn long_base64_and_uri_inputs_exercise_architecture_dispatch_with_both_answers() {
    for length in [32, 33, 64, 65] {
        let mut alphabet = vec![b'A'; length];
        assert!(http_headers_simd::all_base64_alphabet(&alphabet));
        alphabet[length - 1] = b'=';
        assert!(!http_headers_simd::all_base64_alphabet(&alphabet));

        let mut path = vec![b'a'; length];
        path[0] = b'/';
        assert!(http_headers_simd::is_simple_uri_path(&path));
        path[length - 1] = b' ';
        assert!(!http_headers_simd::is_simple_uri_path(&path));
    }
}

#[test]
fn afternoon_range_windows_retain_a_single_member_starting_at_lane_zero() {
    for (wire, expected) in [
        ("123456789012345-", true),
        ("-123456789012345", true),
        ("123456789-123456", false),
        ("1-2,3-4,5-6,7-8", true),
    ] {
        assert_eq!(http_headers_simd::scan_byte_range_set(wire.as_bytes(), 0), expected, "{wire}");
    }
}

#[test]
fn afternoon_case_folding_covers_every_byte_in_vector_and_overlapping_tail_lanes() {
    for length in [16, 23, 24, 25, 31, 32, 33, 39, 40, 41, 47, 48, 49, 63, 64, 65] {
        for lane in [0, length / 2, length - 1] {
            for byte in 0..=u8::MAX {
                let mut left = vec![b'a'; length];
                let mut right = vec![b'A'; length];
                left[lane] = byte;
                right[lane] = byte.to_ascii_lowercase();
                assert!(eq_ignore_ascii_case(&left, &right), "{length}/{lane}/{byte}");
                right[lane] = byte ^ 0x20;
                assert_eq!(
                    eq_ignore_ascii_case(&left, &right),
                    byte.is_ascii_alphabetic(),
                    "{length}/{lane}/{byte}"
                );
            }
            for length in [1, 15, 16, 17, 31, 32, 33, 64, 65] {
                let input = vec![b'a'; length];
                assert!(!eq_ignore_ascii_case(&input, &input[..length - 1]));
                assert!(!eq_ignore_ascii_case(&input[..length - 1], &input));
            }
        }
    }
}

#[test]
fn afternoon_all_tail_lengths_preserve_the_first_match_and_invalid_byte() {
    for length in 0..=80 {
        let input = vec![b'a'; length];
        assert_eq!(is_token(&input), length > 0);
        assert_eq!(is_token68(&input), length > 0);
        assert!(is_field_value(&input));
        assert_eq!(find_interesting(&input), None);
        assert_eq!(find_either(&input, b'?', b'!'), None);
        for lane in 0..length {
            let mut input = input.clone();
            input[lane] = b'\n';
            assert!(!is_token(&input));
            assert!(!is_token68(&input));
            assert!(!is_field_value(&input));
            input[lane] = b';';
            assert_eq!(find_interesting(&input), Some(lane));
            assert_eq!(find_either(&input, b';', b'!'), Some(lane));
            assert_eq!(find_either(&input, b'!', b';'), Some(lane));
            if lane + 1 < length {
                input[length - 1] = b';';
                assert_eq!(find_interesting(&input), Some(lane));
                assert_eq!(find_either(&input, b';', b'!'), Some(lane));
            }
        }
    }
}

#[test]
fn afternoon_token68_padding_must_follow_data_and_remain_a_suffix_across_vectors() {
    for length in 0..=80 {
        for data_length in 0..=length {
            let mut input = vec![b'a'; length];
            input[data_length..].fill(b'=');
            assert_eq!(is_token68(&input), data_length != 0, "{length}/{data_length}");
            if data_length < length {
                input[length - 1] = b'a';
                assert_eq!(is_token68(&input), data_length + 1 == length, "{length}/{data_length}");
            }
        }
    }
}

#[cfg(feature = "benchmarking")]
#[test]
fn afternoon_uri_backends_match_the_rfc_byte_grammar_at_every_vector_lane() {
    use http_headers_simd::benchmarking::{is_simple_uri_path_sse2, is_simple_uri_path_sse42, is_simple_uri_path_ssse3};
    use http_headers_simd::is_simple_uri_path;

    for length in [16, 17, 23, 24, 31, 32, 33, 47, 48, 49, 63, 64, 65, 79, 80] {
        for lane in 1..length {
            let mut input = vec![b'a'; length];
            input[0] = b'/';
            for byte in 0..=u8::MAX {
                input[lane] = byte;
                let allowed = byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?#".contains(&byte);
                let expected = allowed && !(lane == 1 && byte == b'/');
                assert_eq!(is_simple_uri_path(&input), expected, "{length}/{lane}/{byte}");
                for actual in [
                    is_simple_uri_path_sse2(&input),
                    is_simple_uri_path_ssse3(&input),
                    is_simple_uri_path_sse42(&input),
                ]
                .into_iter()
                .flatten()
                {
                    assert_eq!(actual, expected, "{length}/{lane}/{byte}");
                }
            }
            input[lane] = b'#';
            if lane + 1 < length {
                input[length - 1] = b'#';
                assert!(!is_simple_uri_path(&input));
                for actual in [
                    is_simple_uri_path_sse2(&input),
                    is_simple_uri_path_ssse3(&input),
                    is_simple_uri_path_sse42(&input),
                ]
                .into_iter()
                .flatten()
                {
                    assert!(!actual, "{length}/{lane}");
                }
            }
        }
    }
}
