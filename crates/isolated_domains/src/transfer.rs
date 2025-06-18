// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Create a requested number of domains.
#[must_use]
pub fn create_domains(num: usize) -> Vec<Domain> {
    //TODO Should this return a custom Struct `Domains` instead?

    (0..num)
        .map(|i| Domain {
            index: i,
            num_domains: num,
        })
        .collect()
}

/// A `Domain` can be thought of as a placement in a system.
///
/// It is used to represent a specific context or environment where data can be processed.
/// For example a NUMA node, a thread, a specific CPU core, or a specific memory region.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Domain {
    index: usize,
    num_domains: usize,
}

impl Domain {
    /// Returns the index of the domain.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Returns the total number of domains.
    #[must_use]
    pub const fn num_domains(&self) -> usize {
        self.num_domains
    }
}

/// The `Transfer` trait is used to transfer data between different domains.
///
/// This is an 'infectious' trait, meaning that when you implement it for a type,
/// all of its fields must also implement `Transfer` and you must call their `transfer` methods.
///
/// # Notes on source
/// The `source` parameter is the domain from which the data is being transferred.
/// If you clone a transferrable type, the `source` might be different for the cloned instance.
pub trait Transfer {
    #[must_use]
    fn transfer(self, source: Domain, destination: Domain) -> impl Future<Output = Self>;
}

#[cfg(test)]
mod tests {
    use super::{Domain, create_domains};

    #[test]
    fn test_create_domains() {
        let domains = create_domains(5);
        assert_eq!(domains.len(), 5);
        for (i, domain) in domains.iter().enumerate() {
            assert_eq!(domain.index(), i);
            assert_eq!(domain.num_domains(), 5);
        }
    }

    #[test]
    fn test_domain() {
        let domain = Domain {
            index: 2,
            num_domains: 4,
        };
        assert_eq!(domain.index(), 2);
        assert_eq!(domain.num_domains(), 4);
    }
}