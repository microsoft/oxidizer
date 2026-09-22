// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-second duration contracts for max-age construction and insertion.

#![cfg(all(feature = "headers-cors", feature = "headers-cache-control", feature = "headers-security"))]

use std::time::Duration;

use http_headers::headers::{AccessControlMaxAgeOwned, CacheControlOwned, StrictTransportSecurityOwned};
use http_headers::{DecodeError, DecodeErrorKind, FieldName};

const FRACTIONAL: [Duration; 4] = [
    Duration::from_nanos(1),
    Duration::from_nanos(999_999_999),
    Duration::from_millis(2_500),
    Duration::new(u64::MAX, 1),
];

fn assert_invalid_duration(error: DecodeError, name: &FieldName) {
    assert_eq!(error.header(), name);
    assert_eq!(error.kind(), DecodeErrorKind::InvalidNumber);
    assert_eq!(error.value_index(), None);
}

#[test]
fn fractional_max_ages_are_rejected_without_flooring() {
    for duration in FRACTIONAL {
        let error = AccessControlMaxAgeOwned::from_duration(duration).unwrap_err();
        assert_invalid_duration(error, &FieldName::AccessControlMaxAge);
        assert_eq!(AccessControlMaxAgeOwned::try_from(duration).unwrap_err(), error);

        assert_invalid_duration(
            CacheControlOwned::builder().public().max_age(duration).build().unwrap_err(),
            &FieldName::CacheControl,
        );

        let error = StrictTransportSecurityOwned::new(duration).unwrap_err();
        assert_invalid_duration(error, &FieldName::StrictTransportSecurity);
        assert_eq!(StrictTransportSecurityOwned::builder(duration).build().unwrap_err(), error);
    }
}

#[test]
fn whole_second_max_ages_round_trip_including_numeric_bounds() {
    for seconds in [0, 1, 2, 600, u64::MAX] {
        let duration = Duration::from_secs(seconds);
        let cors = AccessControlMaxAgeOwned::from_duration(duration).unwrap();
        assert_eq!(cors, AccessControlMaxAgeOwned::try_from(duration).unwrap());
        assert_eq!(cors.seconds(), seconds);
        assert_eq!(Duration::from(cors), duration);
        assert_eq!(cors.into_field_value().as_bytes(), seconds.to_string().as_bytes());

        let cache = CacheControlOwned::builder().max_age(duration).build().unwrap();
        assert_eq!(cache.max_age(), Some(duration));
        let hsts = StrictTransportSecurityOwned::builder(duration).build().unwrap();
        assert_eq!(hsts.max_age(), duration);
        assert_eq!(hsts.as_field_value().as_bytes(), format!("max-age={seconds}").as_bytes());
    }
}

#[cfg(feature = "http")]
#[test]
fn invalid_duration_insertion_and_append_preserve_existing_headers() {
    use http::{HeaderMap, HeaderValue, header};
    use http_headers::sink::{FieldSink, FieldSinkExt, InsertErrorKind};

    let mut map = HeaderMap::new();
    map.append(header::CACHE_CONTROL, HeaderValue::from_static("public"));
    map.append(header::CACHE_CONTROL, HeaderValue::from_static("max-age=60"));
    map.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    let original = map.clone();

    for duration in FRACTIONAL {
        let builder = CacheControlOwned::builder().public().max_age(duration);
        assert_eq!(
            map.set_cache_control(builder.clone()).unwrap_err().kind(),
            InsertErrorKind::InvalidValue
        );
        assert_eq!(map, original);
        assert_eq!(
            map.append_encoded(&FieldName::CacheControl, builder).unwrap_err().kind(),
            InsertErrorKind::InvalidValue
        );
        assert_eq!(map, original);
        assert_eq!(
            map.set_access_control_max_age_duration(duration).unwrap_err().kind(),
            InsertErrorKind::InvalidValue
        );
        assert_eq!(map, original);
    }

    for seconds in [0, 2, u64::MAX] {
        let duration = Duration::from_secs(seconds);
        map.set_access_control_max_age_duration(duration).unwrap();
        assert_eq!(map[header::ACCESS_CONTROL_MAX_AGE].as_bytes(), seconds.to_string().as_bytes());
        map.set_cache_control(CacheControlOwned::builder().max_age(duration)).unwrap();
        assert_eq!(map[header::CACHE_CONTROL].as_bytes(), format!("max-age={seconds}").as_bytes());
    }
}
