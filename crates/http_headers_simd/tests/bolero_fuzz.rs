// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded public-API properties for the SIMD byte scanners.

use std::time::Duration;

use http_headers_simd::{eq_ignore_ascii_case, find_interesting, is_field_value, is_token, is_token68};

const BOUNDED_ITERATIONS: usize = 4_096;
const BOUNDED_TEST_TIME: Duration = Duration::from_millis(400);
const MAX_INPUT_LENGTH: usize = 1_024;

#[test]
#[cfg_attr(miri, ignore = "Bolero corpus replay requires filesystem access unavailable under Miri isolation")]
fn public_scanners_match_scalar_oracles() {
    bolero::check!()
        .with_iterations(BOUNDED_ITERATIONS)
        .with_test_time(BOUNDED_TEST_TIME)
        .with_type::<(Vec<u8>, Vec<u8>)>()
        .for_each(|(left, right)| {
            let left = &left[..left.len().min(MAX_INPUT_LENGTH)];
            let right = &right[..right.len().min(MAX_INPUT_LENGTH)];

            let token = !left.is_empty()
                && left
                    .iter()
                    .copied()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));
            let token68_data_end = left.iter().position(|byte| *byte == b'=').unwrap_or(left.len());
            let token68 = token68_data_end != 0
                && left[..token68_data_end]
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/".contains(byte))
                && left[token68_data_end..].iter().all(|byte| *byte == b'=');
            let field_value = left.iter().copied().all(|byte| byte == b'\t' || byte >= b' ' && byte != 0x7f);
            let equal = left.len() == right.len() && left.iter().zip(right).all(|(left, right)| left.eq_ignore_ascii_case(right));
            let interesting = left.iter().position(|byte| b",;\"\\ \t".contains(byte));

            assert_eq!(is_token(left), token);
            assert_eq!(is_token68(left), token68);
            assert_eq!(is_field_value(left), field_value);
            assert_eq!(eq_ignore_ascii_case(left, right), equal);
            assert_eq!(find_interesting(left), interesting);
        });
}

#[test]
fn scanner_boundary_regression_seeds() {
    for length in [0, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, MAX_INPUT_LENGTH] {
        let mut bytes = vec![b'a'; length];
        assert_eq!(is_token(&bytes), length != 0);
        assert_eq!(is_token68(&bytes), length != 0);
        assert!(is_field_value(&bytes));
        assert_eq!(find_interesting(&bytes), None);

        if let Some(last) = bytes.last_mut() {
            *last = b';';
            assert!(!is_token(&bytes));
            assert_eq!(find_interesting(&bytes), Some(length - 1));
        }
    }
}
