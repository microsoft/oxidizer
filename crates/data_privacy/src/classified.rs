// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::DataClass;

/// Represents a container that holds classified state.
///
/// Types that implement this trait are containers of classified data. They hide an
/// instance they are given to ensure it is handled carefully throughout the application.
/// Although instances are encapsulated, it's possible to extract the instances when
/// classification is no longer needed.
///
/// You rarely implement this trait by hand, instead use the [`classified`](data_privacy_macros::classified) macro to generate
/// classified types automatically.
///
/// # Ancillary Traits
///
/// Types that implement the [`Classified`] trait should generally also implement the [`RedactedDebug`](crate::RedactedDebug),
/// [`RedactedDisplay`](crate::RedactedDisplay), and [`RedactedToString`](crate::RedactedToString) traits. These traits ensure
/// that when classified data is logged or printed, the sensitive information is redacted according to the configured
/// redaction policies.
///
/// Types that implement the [`Classified`] trait should generally not implement the [`Display`](core::fmt::Display) trait, and if they implement
/// the [`Debug`] trait, the implementation should avoid exposing the classified payload.
///
/// # Example
///
/// ```rust
/// use data_privacy::{Classified, DataClass};
///
/// #[derive(Debug)]
/// struct Person {
///    name: String,
///    address: String,
/// }
///
/// impl Person {
///     fn new(name: String, address: String) -> Self {
///         Self { name, address }
///     }
/// }
///
/// // A classified wrapper is usually a newtype around the payload.
/// #[derive(Debug)]
/// struct ClassifiedPerson(Person);
///
/// impl ClassifiedPerson {
///    pub fn new(person: Person) -> Self {
///        Self(person)
///    }
/// }
///
/// impl Classified for ClassifiedPerson {
///     fn data_class(&self) -> DataClass {
///         DataClass::new("example_taxonomy", "classified_person")
///     }
/// }
///
/// let person = Person::new("John Doe".to_string(), "123 Main St".to_string());
/// let classified = ClassifiedPerson::new(person);
/// assert_eq!(classified.data_class().taxonomy(), "example_taxonomy");
/// assert_eq!(classified.data_class().name(), "classified_person");
///  ```
pub trait Classified {
    /// Returns the data class of the classified data.
    #[must_use]
    fn data_class(&self) -> DataClass;
}
