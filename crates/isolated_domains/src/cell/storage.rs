// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::Domain;

/// Internal type used for storing data in a domain-aware manner.
/// This type is used to store data for each domain, allowing for domain-specific storage.
///
/// This type is used by [`Trc<T>`] to manage the data for each domain.
#[derive(Debug)]
pub struct Storage<T> {
    data: Vec<Option<T>>,
}

impl<T> Storage<T> {
    /// Creates a new `Storage` instance with an empty vector.
    #[must_use]
    pub const fn new() -> Self {
        Self { data: Vec::new() }
    }

    ///Replaces the data for the given domain with the provided value.
    /// Returns the previous value if it existed, otherwise returns `None`.
    pub fn replace(&mut self, domain: Domain, value: T) -> Option<T> {
        self.resize(domain.num_domains());

        self.data[domain.index()].replace(value)
    }

    fn resize(&mut self, num_domains: usize) {
        if self.data.len() < num_domains {
            self.data.resize_with(num_domains, || None);
        }
    }
}

impl<T> Default for Storage<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Storage<T>
where
    T: Clone,
{
    /// Clone and gets the data for the given domain if it exists.
    /// Returns `None` if the data does not exist for that domain.
    #[must_use]
    pub fn get_clone(&self, domain: Domain) -> Option<T> {
        self.data
            .get(domain.index())
            .and_then(std::clone::Clone::clone)
    }
}

#[cfg(test)]
mod tests {
    use crate::create_domains;

    #[test]
    fn test_get_clone() {
        use super::Storage;

        let domains = create_domains(1);

        let mut storage = Storage::new();
        let domain = domains[0];

        assert!(storage.get_clone(domain).is_none());

        storage.replace(domain, "Hello".to_string());
        assert_eq!(storage.get_clone(domain), Some("Hello".to_string()));
    }
}