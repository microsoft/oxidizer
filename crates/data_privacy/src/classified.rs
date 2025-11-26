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
/// Types that implement the [`Classified`] trait should generally not implement the [`core::fmt::Display`] trait, and if they implement
/// the [`Debug`] trait, the implementation should avoid exposing the classified payload. Most types should derive the [`ClassifiedDebug`](data_privacy_macros::ClassifiedDebug) macro
/// to get an appropriate implementation of the [`Debug`] trait.
///
/// # Example
///
/// ```rust
/// use data_privacy::{Classified, ClassifiedDebug, DataClass, RedactedDebug, RedactedDisplay, RedactedToString};
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
/// #[derive(ClassifiedDebug, RedactedDebug)]
/// struct ClassifiedPerson(Person);
///
/// impl ClassifiedPerson {
///    pub fn new(person: Person) -> Self {
///        Self(person)
///    }
/// }
///
/// impl Classified for ClassifiedPerson {
///     type Payload = Person;
///
///     fn declassify(self) -> Person {
///         self.0
///     }
///
///     fn as_declassified(&self) -> &Person {
///         &self.0
///     }
///
///     fn as_declassified_mut(&mut self) -> &mut Person {
///         &mut self.0
///     }
///
///     fn data_class(&self) -> DataClass {
///         DataClass::new("example_taxonomy", "classified_person")
///     }
/// }
///  ```
pub trait Classified {
    // /// Visits the payload with the provided operation.
    // fn visit(&self, operation: impl FnOnce(&Self::Payload)) {
    //     operation(self.as_declassified());
    // }
    //
    // /// Visits the payload with the provided operation.
    // fn visit_mut(&mut self, operation: impl FnOnce(&mut Self::Payload)) {
    //     operation(self.as_declassified_mut());
    // }

    /// Returns the data class of the classified data.
    #[must_use]
    fn data_class(&self) -> DataClass;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct ClassifiedExample {
        data: u32,
    }

    impl Classified for ClassifiedExample {
        fn data_class(&self) -> DataClass {
            DataClass::new("example", "classified_example")
        }
    }

    #[test]
    fn test_default_trait_methods() {
        let mut classified = ClassifiedExample { data: 42 };

        // let mut call_count = 0;
        // classified.visit(|value| {
        //     assert_eq!(*value, 42, "Initial value should be 42");
        //     call_count += 1;
        // });
        //
        // assert_eq!(call_count, 1);
        //
        // classified.visit_mut(|value| {
        //     *value = 20;
        // });
        // classified.visit(|value| assert_eq!(*value, 20, "Value should be updated to 20"));

        assert_eq!(classified.data_class().name(), "classified_example");

    }
}
