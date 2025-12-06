// Copyright (c) Microsoft Corporation.

use std::time::SystemTime;

use jiff::Timestamp as TimestampJiff;

use crate::{Error, Timestamp};

pub(super) fn from_jiff(value: TimestampJiff) -> Result<Timestamp, Error> {
    Timestamp::from_system_time(SystemTime::from(value))
}

pub(super) fn to_jiff(value: Timestamp) -> TimestampJiff {
    value
        .to_system_time()
        .try_into()
        .expect("conversion of tick timestamp to jiff stamp shall never fail")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use jiff::SignedDuration;

    use super::*;

    #[test]
    fn from_jiff_min() {
        from_jiff(TimestampJiff::MIN).unwrap_err();
    }

    #[test]
    fn from_jiff_zero() {
        let timestamp = from_jiff(TimestampJiff::from_second(0).unwrap()).unwrap();

        assert_eq!(timestamp.to_system_time(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn from_jiff_max() {
        let jiff = to_jiff(from_jiff(TimestampJiff::MAX).unwrap());

        assert_eq!(TimestampJiff::MAX.as_second(), jiff.as_second());
    }

    #[test]
    fn from_to_jiff() {
        let timestamp =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_millis(123))
                .unwrap();
        assert_eq!(timestamp, from_jiff(to_jiff(timestamp)).unwrap());
    }

    #[test]
    fn from_jiff_overflow() {
        let ts = TimestampJiff::from_duration(SignedDuration::from_secs(-10)).unwrap();
        let error = from_jiff(ts).unwrap_err();
        assert_eq!(
            "negative system time cannot be converted to timestamp",
            error.to_string()
        );
    }
}
