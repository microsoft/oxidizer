// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public timestamp parsing, formatting, conversion, and serialization contracts.

#![cfg(feature = "fmt")]

use std::error::Error as _;
use std::fmt::Debug;
use std::hash::Hash;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use jiff::Timestamp;
#[cfg(feature = "test-util")]
use tick::Clock;
use tick::SystemTimeExt;
use tick::fmt::{EcmaScript, Iso8601, Rfc2822, UnixSeconds};

#[test]
fn format_types_expose_their_documented_value_traits() {
    static_assertions::assert_impl_all!(EcmaScript: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, FromStr);
    static_assertions::assert_impl_all!(Iso8601: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, FromStr);
    static_assertions::assert_impl_all!(Rfc2822: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, FromStr);
    static_assertions::assert_impl_all!(UnixSeconds: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, FromStr);
}

#[cfg(feature = "test-util")]
#[test]
fn system_time_as_supports_every_tick_format_for_normalized_clock_data() {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_722_980_600);
    let clock = Clock::new_frozen_at(time);

    assert_eq!(SystemTime::from(clock.system_time_as::<Iso8601>()), time);
    assert_eq!(SystemTime::from(clock.system_time_as::<EcmaScript>()), time);
    assert_eq!(SystemTime::from(clock.system_time_as::<Rfc2822>()), time);
    assert_eq!(SystemTime::from(clock.system_time_as::<UnixSeconds>()), time);
}

#[test]
fn formats_do_not_hide_fallible_cross_format_conversions() {
    static_assertions::assert_not_impl_any!(Iso8601: From<EcmaScript>, From<Rfc2822>, From<UnixSeconds>);
    static_assertions::assert_not_impl_any!(EcmaScript: From<Iso8601>, From<Rfc2822>, From<UnixSeconds>);
    static_assertions::assert_not_impl_any!(Rfc2822: From<EcmaScript>, From<Iso8601>, From<UnixSeconds>);
    static_assertions::assert_not_impl_any!(UnixSeconds: From<EcmaScript>, From<Iso8601>, From<Rfc2822>);
}

#[test]
fn unix_epoch_constants_all_represent_the_unix_epoch() {
    assert_eq!(
        [
            SystemTime::from(Iso8601::UNIX_EPOCH),
            SystemTime::from(Rfc2822::UNIX_EPOCH),
            SystemTime::from(UnixSeconds::UNIX_EPOCH),
            SystemTime::from(EcmaScript::UNIX_EPOCH),
        ],
        [SystemTime::UNIX_EPOCH; 4]
    );
}

#[test]
fn max_constants_match_each_format_resolution() {
    let iso_max = SystemTime::from(Iso8601::MAX);
    let rfc_max = SystemTime::from(Rfc2822::MAX);
    let ecma_max = SystemTime::from(EcmaScript::MAX);
    let unix_max = SystemTime::from(UnixSeconds::MAX);

    assert_eq!(iso_max, rfc_max);
    assert!(unix_max <= ecma_max);
    assert!(ecma_max <= iso_max);
    assert_eq!(Iso8601::MAX.to_string(), "9999-12-30T22:00:00.9999999Z");
    assert_eq!(Rfc2822::MAX.to_string(), "Thu, 30 Dec 9999 22:00:00 GMT");
    assert_eq!(EcmaScript::MAX.to_string(), "9999-12-30T22:00:00.999Z");
    assert_eq!(UnixSeconds::MAX.to_string(), "253402207200");
}

#[test]
fn ecmascript_has_fixed_millisecond_width() {
    let epoch = EcmaScript::try_from(SystemTime::UNIX_EPOCH).unwrap();
    let fractional = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::from_micros(1)).unwrap();

    assert_eq!(epoch.to_string(), "1970-01-01T00:00:00.000Z");
    assert_eq!(fractional.to_string(), "1970-01-01T00:00:00.000Z");
    assert_eq!(epoch.to_string().len(), fractional.to_string().len());
}

#[test]
fn ecmascript_renders_and_canonicalizes_milliseconds() {
    let rendered = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::from_millis(10_123_456)).unwrap();
    let a = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::new(8, 123_400_000)).unwrap();
    let b = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::new(8, 123_900_000)).unwrap();

    assert_eq!(rendered.to_string(), "1970-01-01T02:48:43.456Z");
    assert_eq!(a, b);
    assert_eq!(a.to_string(), "1970-01-01T00:00:08.123Z");
}

#[test]
fn ecmascript_truncates_sub_milliseconds_without_rounding() {
    let formatted = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::new(8, 999_999_999)).unwrap();

    assert_eq!(formatted.to_string(), "1970-01-01T00:00:08.999Z");
}

#[test]
fn ecmascript_pre_epoch_precision_floors_to_the_civil_millisecond() {
    let ecma: EcmaScript = "1969-12-31T23:59:59.999999999Z".parse().unwrap();

    assert_eq!(ecma.to_string(), "1969-12-31T23:59:59.999Z");
    assert_eq!(ecma, "1969-12-31T23:59:59.999Z".parse().unwrap());
}

#[test]
fn ecmascript_parse_normalizes_to_utc_and_fixed_precision() {
    let at_epoch: EcmaScript = "1970-01-01T00:00:00Z".parse().unwrap();
    let offset: EcmaScript = "2024-08-06T14:30:00-07:00".parse().unwrap();

    assert_eq!(at_epoch, EcmaScript::UNIX_EPOCH);
    assert_eq!(SystemTime::from(at_epoch), SystemTime::UNIX_EPOCH);
    assert_eq!(offset.to_string(), "2024-08-06T21:30:00.000Z");
}

#[test]
fn ecmascript_parse_rejects_invalid_and_overflow_values() {
    assert!(!"date".parse::<EcmaScript>().unwrap_err().is_out_of_range());
    assert!(
        !"10000-12-30T22:00:00.999999999Z"
            .parse::<EcmaScript>()
            .unwrap_err()
            .is_out_of_range()
    );
    assert!(
        "+010000-12-30T22:00:00.999999999Z"
            .parse::<EcmaScript>()
            .unwrap_err()
            .is_out_of_range()
    );
}

#[test]
fn ecmascript_max_truncates_to_milliseconds() {
    let parsed: EcmaScript = "9999-12-30T22:00:00.999999999Z".parse().unwrap();

    assert_eq!(parsed, EcmaScript::MAX);
    assert_eq!(parsed.to_string(), "9999-12-30T22:00:00.999Z");
}

#[test]
fn ecmascript_rejects_system_time_above_its_range() {
    let overflow = SystemTime::from(Timestamp::MAX) + Duration::from_hours(24);

    assert!(EcmaScript::try_from(overflow).unwrap_err().is_out_of_range());
}

#[cfg(not(windows))]
#[test]
fn ecmascript_negative_year_uses_expanded_form() {
    let ecma: EcmaScript = "-009999-01-02T01:59:59Z".parse().unwrap();
    let rendered = ecma.to_string();

    assert_eq!(rendered, "-009999-01-02T01:59:59.000Z");
    assert_eq!(rendered.parse::<EcmaScript>().unwrap(), ecma);
}

#[test]
fn ecmascript_expanded_year_parsing_matches_the_platform_system_time_range() {
    let input = "-000500-01-01T00:00:00Z";
    let parsed = input.parse::<EcmaScript>();
    let timestamp: Timestamp = input.parse().unwrap();
    let expected = timestamp.as_duration();
    let expected = if expected.is_negative() {
        SystemTime::UNIX_EPOCH.checked_sub(expected.unsigned_abs())
    } else {
        SystemTime::UNIX_EPOCH.checked_add(expected.unsigned_abs())
    };

    match expected {
        Some(expected) => {
            let ecma = parsed.unwrap();
            assert_eq!(ecma.to_string(), "-000500-01-01T00:00:00.000Z");
            assert_eq!(SystemTime::from(ecma), expected);
        }
        None => {
            parsed.unwrap_err();
        }
    }
}

#[test]
fn iso8601_parse_errors_preserve_a_source() {
    let error = "date".parse::<Iso8601>().unwrap_err();

    assert!(error.source().is_some());
    assert!(!error.to_string().is_empty());
}

#[test]
fn iso8601_parses_displays_and_converts_public_values() {
    let epoch: Iso8601 = "1970-01-01T00:00:00Z".parse().unwrap();
    let hour: Iso8601 = "1970-01-01T01:00:00Z".parse().unwrap();
    let max: Iso8601 = "9999-12-30T22:00:00.9999999Z".parse().unwrap();

    assert_eq!(epoch, Iso8601::UNIX_EPOCH);
    assert_eq!(SystemTime::from(epoch), SystemTime::UNIX_EPOCH);
    assert_eq!(hour.to_string(), "1970-01-01T01:00:00Z");
    assert_eq!(SystemTime::from(hour), SystemTime::UNIX_EPOCH + Duration::from_hours(1));
    assert_eq!(max.to_string(), "9999-12-30T22:00:00.9999999Z");
}

#[test]
fn iso8601_rejects_year_overflow() {
    let error = "+010000-12-30T22:00:00.999999999Z".parse::<Iso8601>().unwrap_err();

    assert!(error.is_out_of_range());
    assert!(error.source().unwrap().downcast_ref::<jiff::Error>().is_some());
}

#[test]
fn iso8601_round_trips_system_time() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_hours(1);
    let iso = Iso8601::try_from(at).unwrap();

    assert_eq!(SystemTime::from(iso), at);
}

#[test]
fn iso8601_classifies_system_time_above_its_range() {
    let overflow = SystemTime::from(Timestamp::MAX) + Duration::from_hours(24);
    let error = Iso8601::try_from(overflow).unwrap_err();

    assert!(error.is_out_of_range());
    assert!(error.source().unwrap().downcast_ref::<jiff::Error>().is_some());
}

#[test]
fn iso8601_trims_leap_seconds() {
    let stamp: Iso8601 = "1990-12-31T23:59:60Z".parse().unwrap();

    assert_eq!(stamp.to_string(), "1990-12-31T23:59:59Z");
}

#[test]
fn iso8601_uses_the_platform_resolution_without_drift() {
    let parsed: Iso8601 = "1970-01-01T00:00:00.000000001Z".parse().unwrap();
    let system_time = SystemTime::from(parsed);
    let round_tripped = Iso8601::try_from(system_time).unwrap();

    assert_eq!(round_tripped.to_string(), "1970-01-01T00:00:00Z");
    assert_eq!(SystemTime::from(round_tripped), system_time);
    assert_eq!(Iso8601::try_from(SystemTime::from(round_tripped)).unwrap(), round_tripped);
}

#[test]
fn iso8601_rounds_to_seven_fractional_digits() {
    let parsed: Iso8601 = "1970-01-01T00:00:08.999999999Z".parse().unwrap();
    let after_epoch = Iso8601::try_from(SystemTime::UNIX_EPOCH + Duration::new(8, 999_999_999)).unwrap();
    let before_epoch = Iso8601::try_from(SystemTime::UNIX_EPOCH - Duration::new(8, 999_999_999)).unwrap();

    assert_eq!(parsed.to_string(), "1970-01-01T00:00:08.9999999Z");
    assert_eq!(after_epoch.to_string(), "1970-01-01T00:00:08.9999999Z");
    assert_eq!(before_epoch.to_string(), "1969-12-31T23:59:51.0000001Z");
}

#[test]
fn iso8601_parsing_agrees_with_the_platform_system_time_range() {
    for input in ["1000-01-01T00:00:00Z", "0001-01-01T00:00:00Z", "-000500-01-01T00:00:00Z"] {
        let timestamp: Timestamp = input.parse().unwrap();
        let offset = timestamp.as_duration();
        let expected = if offset.is_negative() {
            SystemTime::UNIX_EPOCH.checked_sub(offset.unsigned_abs())
        } else {
            SystemTime::UNIX_EPOCH.checked_add(offset.unsigned_abs())
        };
        let parsed = input.parse::<Iso8601>();

        match expected {
            Some(expected) => assert_eq!(SystemTime::from(parsed.unwrap()), expected, "{input}"),
            None => assert_eq!(
                parsed.unwrap_err().to_string(),
                "the instant is outside the range that `SystemTime` can represent on this platform",
                "{input}"
            ),
        }
    }
}

#[test]
fn iso8601_parsed_values_convert_without_panicking() {
    for input in ["1601-01-01T00:00:00Z", "1970-01-01T00:00:00Z", "2024-08-06T21:30:00Z"] {
        let iso: Iso8601 = input.parse().unwrap();

        assert_eq!(Iso8601::try_from(SystemTime::from(iso)).unwrap(), iso, "{input}");
    }
}

#[test]
fn iso8601_supports_the_filetime_epoch() {
    let iso: Iso8601 = "1601-01-01T00:00:00Z".parse().unwrap();

    assert_eq!(iso.to_string(), "1601-01-01T00:00:00Z");
    assert_eq!(SystemTime::from(iso), SystemTime::UNIX_EPOCH - Duration::from_hours(3_234_576));
}

#[test]
fn rfc2822_parses_displays_and_normalizes_public_values() {
    let epoch: Rfc2822 = "Thu, 1 Jan 1970 00:00:00 GMT".parse().unwrap();
    let second: Rfc2822 = "Thu, 1 Jan 1970 00:00:01 -0000".parse().unwrap();
    let hour: Rfc2822 = "Thu, 01 Jan 1970 01:00:00 GMT".parse().unwrap();

    assert_eq!(SystemTime::from(epoch), SystemTime::UNIX_EPOCH);
    assert_eq!(SystemTime::from(second), SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    assert_eq!(hour.to_string(), "Thu, 01 Jan 1970 01:00:00 GMT");
    assert_eq!(SystemTime::from(hour), SystemTime::UNIX_EPOCH + Duration::from_hours(1));
}

#[test]
fn rfc2822_display_is_total_for_publicly_constructed_values() {
    for input in ["Thu, 01 Jan 1970 00:00:00 GMT", "Tue, 06 Aug 2024 21:30:00 GMT"] {
        let rfc: Rfc2822 = input.parse().unwrap();

        assert_eq!(rfc.to_string(), input);
        assert_eq!(format!("{rfc}"), input);
    }

    assert_eq!(Rfc2822::MAX.to_string(), "Thu, 30 Dec 9999 22:00:00 GMT");
}

#[test]
fn rfc2822_handles_leap_years_and_leap_seconds() {
    let leap_year: Rfc2822 = "Tue, 29 Feb 2000 01:00:00 GMT".parse().unwrap();
    let leap_second: Rfc2822 = "Mon, 31 Dec 1990 23:59:60 GMT".parse().unwrap();

    assert_eq!(leap_year.to_string(), "Tue, 29 Feb 2000 01:00:00 GMT");
    assert_eq!(
        SystemTime::from(leap_year)
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        951_786_000
    );
    assert_eq!(leap_second.to_string(), "Mon, 31 Dec 1990 23:59:59 GMT");
}

#[test]
fn rfc2822_rejects_invalid_and_overflow_values() {
    "date".parse::<Rfc2822>().unwrap_err();
    "Thu, 30 Dec 10000 22:00:00 GMT".parse::<Rfc2822>().unwrap_err();
}

#[test]
fn rfc2822_accepts_its_public_maximum() {
    let stamp: Rfc2822 = "Thu, 30 Dec 9999 22:00:00 GMT".parse().unwrap();

    assert_eq!(stamp.to_string(), "Thu, 30 Dec 9999 22:00:00 GMT");
    assert!(stamp <= Rfc2822::MAX);
}

#[test]
fn rfc2822_rejects_unencodable_negative_years_when_system_time_can_express_them() {
    let before_year_zero = SystemTime::UNIX_EPOCH.checked_sub(Duration::from_secs(62_167_219_201));

    if let Some(before_year_zero) = before_year_zero {
        let error = Rfc2822::try_from(before_year_zero).unwrap_err();
        assert!(error.is_out_of_range());
        assert!(error.source().is_none());
    }

    assert_eq!(Rfc2822::try_from(SystemTime::UNIX_EPOCH).unwrap(), Rfc2822::UNIX_EPOCH);
}

#[test]
fn rfc2822_classifies_system_time_above_its_range() {
    let overflow = SystemTime::from(Timestamp::MAX) + Duration::from_hours(24);

    assert!(Rfc2822::try_from(overflow).unwrap_err().is_out_of_range());
}

#[test]
fn rfc2822_parsing_agrees_with_the_platform_system_time_range() {
    let input = "Wed, 01 Jan 1000 00:00:00 GMT";
    let timestamp: Timestamp = "1000-01-01T00:00:00Z".parse().unwrap();
    let offset = timestamp.as_duration();
    let expected = SystemTime::UNIX_EPOCH.checked_sub(offset.unsigned_abs());
    let parsed = input.parse::<Rfc2822>();

    match expected {
        Some(expected) => assert_eq!(SystemTime::from(parsed.unwrap()), expected),
        None => assert_eq!(
            parsed.unwrap_err().to_string(),
            "the instant is outside the range that `SystemTime` can represent on this platform"
        ),
    }
}

#[test]
fn unix_seconds_epoch_and_max_have_their_documented_values() {
    assert_eq!(UnixSeconds::UNIX_EPOCH.to_secs(), 0);
    assert_eq!(SystemTime::from(UnixSeconds::UNIX_EPOCH), SystemTime::UNIX_EPOCH);
    assert!(UnixSeconds::UNIX_EPOCH < UnixSeconds::MAX);
    assert_eq!(UnixSeconds::MAX.to_secs(), 253_402_207_200);
}

#[test]
fn unix_seconds_rejects_pre_epoch_system_time() {
    assert!(
        UnixSeconds::try_from(SystemTime::UNIX_EPOCH - Duration::from_secs(1))
            .unwrap_err()
            .is_out_of_range()
    );
    assert_eq!(UnixSeconds::try_from(SystemTime::UNIX_EPOCH).unwrap(), UnixSeconds::UNIX_EPOCH);
    assert_eq!(
        UnixSeconds::try_from(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
            .unwrap()
            .to_secs(),
        1
    );
}

#[test]
fn unix_seconds_constructors_validate_the_public_range() {
    assert_eq!(UnixSeconds::from_secs(10).unwrap().to_secs(), 10);
    let duration_error = UnixSeconds::try_from(Duration::MAX).unwrap_err();
    assert!(duration_error.is_out_of_range());
    assert!(duration_error.source().is_none());
    let seconds_error = UnixSeconds::from_secs(u64::MAX).unwrap_err();
    assert!(seconds_error.is_out_of_range());
    assert!(seconds_error.source().is_none());
}

#[test]
fn unix_seconds_parses_displays_and_converts() {
    let epoch: UnixSeconds = "0".parse().unwrap();
    let hour: UnixSeconds = "3600".parse().unwrap();
    let ten = UnixSeconds::from_secs(10).unwrap();

    assert_eq!(SystemTime::from(epoch), SystemTime::UNIX_EPOCH);
    assert_eq!(hour.to_string(), "3600");
    assert_eq!(SystemTime::from(ten), SystemTime::UNIX_EPOCH + Duration::from_secs(10));
}

#[test]
fn unix_seconds_rejects_invalid_and_overflow_values() {
    assert!(!"date".parse::<UnixSeconds>().unwrap_err().is_out_of_range());
    assert!("99999999999999999999999".parse::<UnixSeconds>().unwrap_err().is_out_of_range());
}

#[test]
fn unix_seconds_round_trips_through_text_and_system_time() {
    let value: UnixSeconds = "9999".parse().unwrap();
    assert_eq!(value.to_string().parse::<UnixSeconds>().unwrap(), value);

    let system_time = SystemTime::from(value);
    assert_eq!(system_time, SystemTime::UNIX_EPOCH + Duration::from_secs(9_999));
    assert_eq!(UnixSeconds::try_from(system_time).unwrap(), value);
    assert_eq!(UnixSeconds::try_from(SystemTime::from(UnixSeconds::MAX)).unwrap(), UnixSeconds::MAX);
}

#[test]
fn unix_seconds_canonicalizes_fractional_seconds() {
    let a = UnixSeconds::try_from(Duration::new(9_999, 400_000_000)).unwrap();
    let b = UnixSeconds::try_from(Duration::new(9_999, 900_000_000)).unwrap();
    let fractional_max = UnixSeconds::try_from(Duration::new(253_402_207_200, 999_999_999)).unwrap();

    assert_eq!(a, b);
    assert_eq!(a.to_string(), "9999");
    assert_eq!(fractional_max, UnixSeconds::MAX);
}

#[test]
fn system_time_extension_formats_iso8601_and_ecmascript() {
    assert_eq!(SystemTime::UNIX_EPOCH.display_iso_8601().to_string(), "1970-01-01T00:00:00Z");
    assert_eq!(
        (SystemTime::UNIX_EPOCH + Duration::from_hours(1)).display_iso_8601().to_string(),
        "1970-01-01T01:00:00Z"
    );
    assert_eq!(SystemTime::UNIX_EPOCH.display_ecmascript().to_string(), "1970-01-01T00:00:00.000Z");
    assert_eq!(
        (SystemTime::UNIX_EPOCH + Duration::from_hours(1)).display_ecmascript().to_string(),
        "1970-01-01T01:00:00.000Z"
    );
}

#[test]
fn system_time_extension_handles_pre_epoch_and_saturating_values() {
    assert_eq!(
        (SystemTime::UNIX_EPOCH - Duration::from_secs(1)).display_ecmascript().to_string(),
        "1969-12-31T23:59:59.000Z"
    );

    let above_max = SystemTime::from(Timestamp::MAX) + Duration::from_secs(12_345);
    assert_eq!(above_max.display_iso_8601().to_string(), "9999-12-30T22:00:00.999999999Z");
    assert_eq!(above_max.display_ecmascript().to_string(), "9999-12-30T22:00:00.999Z");
}

#[cfg(not(windows))]
#[test]
fn system_time_extension_saturates_below_the_minimum() {
    let below_min = SystemTime::UNIX_EPOCH - Duration::from_secs(400_000_000_000);

    assert_eq!(below_min.display_ecmascript().to_string(), "-009999-01-02T01:59:59.000Z");
}

#[cfg(feature = "serde")]
mod serde_tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Dates {
        iso: Iso8601,
        rfc: Rfc2822,
        unix: UnixSeconds,
        ecma: EcmaScript,
    }

    #[test]
    fn all_formats_serialize_with_their_public_wire_shapes() {
        let dates = Dates {
            iso: "1970-01-01T02:48:43.456Z".parse().unwrap(),
            rfc: "Thu, 01 Jan 1970 02:48:43 GMT".parse().unwrap(),
            unix: "10123".parse().unwrap(),
            ecma: "1970-01-01T02:48:43.456Z".parse().unwrap(),
        };

        assert_eq!(
            serde_json::to_string(&dates).unwrap(),
            r#"{"iso":"1970-01-01T02:48:43.456Z","rfc":"Thu, 01 Jan 1970 02:48:43 GMT","unix":10123,"ecma":"1970-01-01T02:48:43.456Z"}"#
        );
        assert_eq!(
            format!("iso: {}, unix: {}, rfc: {}, ecma: {}", dates.iso, dates.unix, dates.rfc, dates.ecma),
            "iso: 1970-01-01T02:48:43.456Z, unix: 10123, rfc: Thu, 01 Jan 1970 02:48:43 GMT, ecma: 1970-01-01T02:48:43.456Z"
        );
    }

    #[test]
    fn all_formats_round_trip_through_json() {
        let dates = Dates {
            iso: "1970-01-01T02:48:43Z".parse().unwrap(),
            rfc: "Thu, 01 Jan 1970 02:48:43 GMT".parse().unwrap(),
            unix: "10123".parse().unwrap(),
            ecma: "1970-01-01T02:48:43Z".parse().unwrap(),
        };

        let json = serde_json::to_string(&dates).unwrap();

        assert_eq!(serde_json::from_str::<Dates>(&json).unwrap(), dates);
    }

    #[test]
    fn each_format_round_trips_through_json() {
        let iso: Iso8601 = "1970-01-01T01:00:00Z".parse().unwrap();
        let rfc: Rfc2822 = "Thu, 1 Jan 1970 01:00:00 GMT".parse().unwrap();
        let unix: UnixSeconds = "9999".parse().unwrap();
        let ecma: EcmaScript = "1970-01-01T01:00:00Z".parse().unwrap();

        assert_eq!(serde_json::from_str::<Iso8601>(&serde_json::to_string(&iso).unwrap()).unwrap(), iso);
        assert_eq!(serde_json::from_str::<Rfc2822>(&serde_json::to_string(&rfc).unwrap()).unwrap(), rfc);
        assert_eq!(
            serde_json::from_str::<UnixSeconds>(&serde_json::to_string(&unix).unwrap()).unwrap(),
            unix
        );
        assert_eq!(
            serde_json::from_str::<EcmaScript>(&serde_json::to_string(&ecma).unwrap()).unwrap(),
            ecma
        );
    }

    #[test]
    fn ecmascript_serde_preserves_canonical_sub_millisecond_identity() {
        let original = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::new(8, 123_456_789)).unwrap();
        let serialized = serde_json::to_string(&original).unwrap();

        assert_eq!(serialized, r#""1970-01-01T00:00:08.123Z""#);
        assert_eq!(serde_json::from_str::<EcmaScript>(&serialized).unwrap(), original);
    }
}
