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
/// The ISO 8601 standard is used worldwide in various applications, ranging
/// from software and digital formats to international communication, ensuring
/// consistency across different systems and regions.
///
/// This type also supports parsing [RFC 3339](https://datatracker.ietf.org/doc/html/rfc3339)
/// timestamps. Output is ISO 8601, and is also valid RFC 3339 for years `0000` through `9999`.
/// An earlier instant uses an ISO 8601 expanded year, such as `-000500-01-01T00:00:00Z`, which
/// is not RFC 3339.
///
/// Examples:
///
/// - `2024-08-06T21:30:00Z` (UTC)
/// - `2024-08-06T14:30:00-07:00` (UTC offset)
///
/// The [`Iso8601`] format is defined in [ISO 8601](https://www.iso.org/obp/ui/#iso:std:iso:8601:-1:ed-1:v1:en).
///
/// # UTC and time zones
///
/// While ISO 8601 can include a UTC offset, the resulting [`Iso8601`] is always represented in the
/// UTC time zone with an offset of `Z`.
///
/// # Serialization and deserialization
///
/// `Iso8601` implements the `Serialize` and `Deserialize` traits from the `serde_core` crate.
/// The system time is serialized as a string using ISO 8601 format.
///
/// The serialization support is available when `serde` feature is enabled.
///
/// # Leap seconds
///
/// If an ISO 8601 string contains a leap second, parsing will succeed and the leap second will be trimmed.
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
/// The upper bound is [`Iso8601::MAX`] (`9999-12-30T22:00:00.9999999Z`). How far back the
/// type reaches depends on the platform: an instant is accepted only if the platform's
/// [`SystemTime`] can represent it, and rejected otherwise, so converting an `Iso8601` back
/// into a [`SystemTime`] always succeeds.
///
/// # Examples
///
/// ## Formatting and parsing - UTC
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
/// ### Formatting and parsing - With UTC offset
///
/// This example demonstrates that the UTC offset is applied to the resulting [`Iso8601`].
/// Note that when formatting the absolute time, the UTC offset is not included in the formatted string.
/// ```
/// use std::time::SystemTime;
///
/// use tick::fmt::Iso8601;
///
/// let iso = "2024-08-06T23:30:00+02:00".parse::<Iso8601>()?;
/// assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z"); // Note that the UTC offset is applied
///
/// let system_time: SystemTime = iso.into();
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Iso8601(Timestamp);

crate::thread_aware_move!(Iso8601);

impl Iso8601 {
    /// The largest value that can be represented by `Iso8601`.
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
        let timestamp = Timestamp::try_from(value).map_err(Error::jiff)?;
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
