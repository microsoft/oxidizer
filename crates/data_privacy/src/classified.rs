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
/// # Example
///
/// ```rust
/// use data_privacy::{Classified, DataClass};
///
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
/// struct ClassifiedPerson {
///     person: Person
/// }
///
/// impl ClassifiedPerson {
///    fn new(person: Person) -> Self {
///        Self { person }
///    }
/// }
///
/// impl Classified<Person> for ClassifiedPerson {
///     fn declassify(self) -> Person {
///         self.person
///     }
///
/// fn as_declassified(&self) -> &Person {
///         &self.person
///     }
///
/// fn as_declassified_mut(&mut self) -> &mut Person {
///         &mut self.person
///     }
///
///     fn data_class(&self) -> DataClass {
///         DataClass::new("example_taxonomy", "classified_person")
///     }
/// }
///  ```
pub trait Classified<T> {
    /// Exfiltrates the payload, allowing it to be used outside the classified context.
    ///
    /// Exfiltration should be done with caution, as it may expose sensitive information.
    ///
    /// # Returns
    /// The original payload.
    #[must_use]
    fn declassify(self) -> T;

    /// Provides a reference to the declassified payload, allowing read access without ownership transfer.
    ///
    /// Exfiltration should be done with caution, as it may expose sensitive information.
    ///
    /// # Returns
    /// A reference to the original payload.
    #[must_use]
    fn as_declassified(&self) -> &T;

    /// Provides a mutable reference to the declassified payload, allowing write access without ownership transfer.
    ///
    /// Exfiltration should be done with caution, as it may expose sensitive information.
    ///
    /// # Returns
    /// A mutable reference to the original payload.
    #[must_use]
    fn as_declassified_mut(&mut self) -> &mut T;

    /// Visits the payload with the provided operation.
    fn visit(&self, operation: impl FnOnce(&T)) {
        operation(self.as_declassified());
    }

    /// Visits the payload with the provided operation.
    fn visit_mut(&mut self, operation: impl FnOnce(&mut T)) {
        operation(self.as_declassified_mut());
    }

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

    impl Classified<u32> for ClassifiedExample {
        fn declassify(self) -> u32 {
            self.data
        }

        fn as_declassified(&self) -> &u32 {
            &self.data
        }

        fn as_declassified_mut(&mut self) -> &mut u32 {
            &mut self.data
        }

        fn data_class(&self) -> DataClass {
            DataClass::new("example", "classified_example")
        }
    }

    #[test]
    fn test_default_trait_methods() {
        let mut classified = ClassifiedExample { data: 42 };
        classified.visit(|value| assert_eq!(*value, 42, "Initial value should be 42"));

        classified.visit_mut(|value| {
            *value = 20;
        });
        classified.visit(|value| assert_eq!(*value, 20, "Value should be updated to 20"));

        assert_eq!(classified.data_class().name(), "classified_example");
        assert_eq!(classified.as_declassified(), &20);
        assert_eq!(classified.declassify(), 20);
    }
}
