// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::SystemTime;

use jiff::{SignedDuration, Timestamp};

use crate::Error;
use crate::fmt::ensure_system_time_representable;

/// Parser and formatter for system time in ISO 8601 format.
///
/// This type also parses
/// [RFC 3339](https://datatracker.ietf.org/doc/html/rfc3339) timestamps. Output
/// uses ISO 8601. For years `0000` through `9999`, the output is also valid RFC
/// 3339. An earlier time uses an expanded year, such as
/// `-000500-01-01T00:00:00Z`. RFC 3339 does not support that form.
///
/// Examples include:
///
/// - `2024-08-06T21:30:00Z` (UTC)
/// - `2024-08-06T14:30:00-07:00` (UTC offset)
///
/// [ISO 8601](https://www.iso.org/obp/ui/#iso:std:iso:8601:-1:ed-1:v1:en)
/// defines this format.
///
/// # UTC and time zones
///
/// Input can include a UTC offset. The stored value and output always use UTC
/// with the `Z` designator.
///
/// # Serialization and deserialization
///
/// `Iso8601` implements `Serialize` and `Deserialize` from `serde_core`.
/// Serialization writes an ISO 8601 string.
///
/// Enable the `serde` feature to use this support.
///
/// # Leap seconds
///
/// Parsing accepts a leap second and changes it to second 59.
///
/// ```
/// use tick::fmt::Iso8601;
///
/// let iso = "1990-12-31T23:59:60Z".parse::<Iso8601>()?;
/// assert_eq!(iso.to_string(), "1990-12-31T23:59:59Z");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Representable range
///
/// The upper bound is [`Iso8601::MAX`]:
/// `9999-12-30T22:00:00.9999999Z`. The platform defines the lower bound.
/// Creation fails if [`SystemTime`] cannot represent the value. Therefore,
/// conversion back to [`SystemTime`] always succeeds.
///
/// # Examples
///
/// ## Parse and format UTC
/// ```
/// use std::time::SystemTime;
///
/// use tick::fmt::Iso8601;
///
/// let iso = "2024-08-06T21:30:00Z".parse::<Iso8601>()?;
/// assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z");
///
/// let system_time: SystemTime = iso.into();
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ### Parse a UTC offset
///
/// Parsing applies the UTC offset. Formatting then writes the absolute time in
/// UTC without the input offset.
/// ```
/// use std::time::SystemTime;
///
/// use tick::fmt::Iso8601;
///
/// let iso = "2024-08-06T23:30:00+02:00".parse::<Iso8601>()?;
/// assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z");
///
/// let system_time: SystemTime = iso.into();
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Iso8601(Timestamp);

crate::thread_aware_move!(Iso8601);

impl Iso8601 {
    /// The largest value that `Iso8601` can represent.
    ///
    /// This represents a Unix system time of `9999-12-30T22:00:00.999999999Z`, which
    /// [`Display`] writes as `9999-12-30T22:00:00.9999999Z` at 100-nanosecond resolution.
    pub const MAX: Self = Self(Timestamp::MAX);

    /// The Unix epoch represented as `Iso8601`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC`.
    pub const UNIX_EPOCH: Self = Self(Timestamp::UNIX_EPOCH);
}

impl FromStr for Iso8601 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = s.parse::<jiff::Timestamp>().map_err(Error::jiff)?;
        Ok(Self(ensure_system_time_representable(timestamp)?))
    }
}

impl Display for Iso8601 {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        // For some scenarios, we need to round the nano precision down by 2 digits because
        // it's too precise for our needs. For example, .NET interop cannot parse ISO 8601
        // timestamps with such high precision.
        let rounded = with_rounded_nanos(self.0);
        Display::fmt(&rounded, f)
    }
}

impl From<Iso8601> for SystemTime {
    fn from(value: Iso8601) -> Self {
        // jiff's conversion panics for an instant this platform's `SystemTime` cannot hold,
        // which no `Iso8601` can be: parsing runs the instant through
        // `ensure_system_time_representable`, `TryFrom<SystemTime>` starts from a
        // `SystemTime` in the first place, and the constants are within range everywhere.
        value.0.into()
    }
}

impl TryFrom<SystemTime> for Iso8601 {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let timestamp = Timestamp::try_from(value).map_err(Error::out_of_range_from)?;
        Ok(Self(timestamp))
    }
}

#[cfg(any(feature = "serde", test))]
impl serde_core::Serialize for Iso8601 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(any(feature = "serde", test))]
impl<'de> serde_core::Deserialize<'de> for Iso8601 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde_core::Deserializer<'de>,
    {
        super::serde::deserialize_from_str(deserializer)
    }
}

fn with_rounded_nanos(timestamp: Timestamp) -> Timestamp {
    let duration = timestamp.duration_since(Timestamp::UNIX_EPOCH);
    let secs = duration.as_secs();
    let nanos = duration.subsec_nanos();
    let nanos = (nanos / 100) * 100;
    let duration = SignedDuration::new(secs, nanos);

    Timestamp::UNIX_EPOCH
        .saturating_add(duration)
        .expect("this can never fail as we know we are within a valid range")
}
