// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Display};
use std::marker::PhantomData;
use std::str::FromStr;

pub(super) fn deserialize_from_str<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde_core::Deserializer<'de>,
    T: FromStr,
    T::Err: Display,
{
    struct FromStrVisitor<T>(PhantomData<T>);

    impl<'de, T> serde_core::de::Visitor<'de> for FromStrVisitor<T>
    where
        T: FromStr,
        T::Err: Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a timestamp string")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde_core::de::Error,
        {
            v.parse().map_err(E::custom)
        }

        fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
        where
            E: serde_core::de::Error,
        {
            v.parse().map_err(E::custom)
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: serde_core::de::Error,
        {
            std::str::from_utf8(v).map_err(E::custom)?.parse().map_err(E::custom)
        }
    }

    deserializer.deserialize_str(FromStrVisitor(PhantomData))
}
