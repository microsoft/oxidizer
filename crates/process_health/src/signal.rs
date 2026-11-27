// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Display, Formatter};

use crate::{Attribute, Health};

/// The health and diagnostics that one publisher contributes to a component.
///
/// A signal is a value, not an event or subscription. Create a publisher through
/// [`Component::publisher`](crate::Component::publisher) or
/// [`Component::publisher_with`](crate::Component::publisher_with). Each call to
/// [`Publisher::publish`](crate::Publisher::publish) replaces that publisher's previous signal.
/// Background aggregation can skip intermediate signals.
///
/// Observe signals through [`Report::signals`](crate::Report::signals). Reports group equal signals
/// and count their publishers. Retained reports and cloned signals do not update themselves.
///
/// # Equality
///
/// Attribute order is not significant.
///
/// # Examples
///
/// ```
/// use health::{Health, Report};
///
/// fn print_degraded(report: &Report) {
///     for (signal, publishers) in report.signals(Health::Degraded) {
///         println!("{publishers} publisher(s): {signal}");
///     }
/// }
/// ```
#[derive(Clone, Debug)]
pub struct Signal {
    state: Health,
    attributes: Box<[Attribute]>,
}

impl Signal {
    pub(crate) fn new(state: Health, attributes: Box<[Attribute]>) -> Self {
        Self { state, attributes }
    }

    pub(crate) fn healthy() -> Self {
        Self::new(Health::Healthy, Box::default())
    }

    /// The individual publisher's state, before component aggregation.
    #[must_use]
    pub const fn state(&self) -> Health {
        self.state
    }

    /// Returns the diagnostic attributes. Iteration order is unspecified.
    pub fn attributes(&self) -> impl Iterator<Item = &Attribute> {
        self.attributes.iter()
    }
}

impl PartialEq for Signal {
    // Attributes compare as a multiset; signals carry few attributes, so the quadratic scan is
    // cheaper than allocating a sorted copy.
    fn eq(&self, other: &Self) -> bool {
        let count = |attributes: &[Attribute], attribute: &Attribute| attributes.iter().filter(|candidate| *candidate == attribute).count();

        self.state == other.state
            && self.attributes.len() == other.attributes.len()
            && self.attributes.iter().all(|attribute| {
                let own = count(&self.attributes, attribute);
                own == count(&other.attributes, attribute)
            })
    }
}

impl Display for Signal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.state, f)?;
        let mut attributes = self.attributes.iter();
        if let Some(first) = attributes.next() {
            write!(f, " ({first}")?;
            for attribute in attributes {
                write!(f, ", {attribute}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(attributes: impl IntoIterator<Item = (&'static str, &'static str)>) -> Signal {
        Signal::new(Health::Degraded, attributes.into_iter().map(Attribute::from).collect())
    }

    #[test]
    fn equality_ignores_attribute_order() {
        assert_eq!(signal([("a", "1"), ("b", "2")]), signal([("b", "2"), ("a", "1")]));
    }

    #[test]
    fn equality_respects_duplicate_counts() {
        assert_ne!(
            signal([("a", "1"), ("a", "1"), ("b", "2")]),
            signal([("a", "1"), ("b", "2"), ("b", "2")])
        );
        assert_ne!(signal([("a", "1")]), signal([("a", "1"), ("b", "2")]));
        assert_ne!(signal([("a", "1"), ("b", "2")]), signal([("a", "1"), ("b", "3")]));
    }

    #[test]
    fn equality_respects_state() {
        assert_ne!(Signal::healthy(), Signal::new(Health::Unhealthy, Box::default()));
    }

    #[test]
    fn display_lists_attributes() {
        assert_eq!(Signal::healthy().to_string(), "Healthy");
        assert_eq!(signal([("a", "1"), ("b", "2")]).to_string(), "Degraded (a=1, b=2)");
    }

    #[test]
    fn display_propagates_short_writes() {
        use std::fmt::Write;

        struct Limited(usize);
        impl Write for Limited {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.0 = self.0.checked_sub(s.len()).ok_or(fmt::Error)?;
                Ok(())
            }
        }
        let signal = signal([("a", "1"), ("b", "2")]);
        let len = signal.to_string().len();
        for capacity in 0..len {
            assert!(write!(Limited(capacity), "{signal}").is_err());
        }
        write!(Limited(len), "{signal}").unwrap();
    }
}
