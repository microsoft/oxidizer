// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Error chain traversal and formatting failure contracts.

use std::error::Error as StdError;
use std::fmt::{self, Write};

use ohno::{EnrichableExt, ErrorExt, ErrorLabel, OhnoCore};

#[derive(Debug)]
struct Link {
    value: u32,
    next: Option<Box<Self>>,
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "link {}", self.value)
    }
}

impl StdError for Link {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.next.as_deref().map(|next| next as &dyn StdError)
    }
}

fn chain() -> Link {
    Link {
        value: 1,
        next: Some(Box::new(Link {
            value: 2,
            next: Some(Box::new(Link { value: 3, next: None })),
        })),
    }
}

struct FailAfter(usize);

impl Write for FailAfter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if s.len() > self.0 {
            return Err(fmt::Error);
        }
        self.0 -= s.len();
        Ok(())
    }
}

#[test]
fn gamma_survivor_core_formatter_propagates_source_and_enrichment_write_failures() {
    let core = OhnoCore::without_backtrace(std::io::Error::other("source")).enrich("context");
    assert_eq!(write!(&mut FailAfter(0), "{core}"), Err(fmt::Error));
    assert_eq!(write!(&mut FailAfter("source".len()), "{core}"), Err(fmt::Error));
    let debug = format!("{core:?}");
    assert!(debug.starts_with("OhnoCore { source: "));
    assert!(debug.contains(", backtrace: "));
    assert!(debug.contains(", enrichment: "));
    assert!(!core.has_backtrace());
    assert!(core.to_string().starts_with("source\n> context"));
}

#[test]
fn gamma_survivor_error_ext_search_skips_nonmatching_sources_and_stops_at_matching_depth() {
    #[ohno::error]
    struct Wrapped;

    let wrapped = Wrapped::caused_by(chain());
    assert_eq!(wrapped.find_source_with::<Link>(|link| link.value == 3).unwrap().value, 3);
    assert!(wrapped.find_source_with::<Link>(|link| link.value == 4).is_none());
    assert_eq!(wrapped.find_source::<Link>().unwrap().value, 1);
}

#[test]
fn gamma_survivor_error_labels_skip_unrecognized_links_and_preserve_root_order() {
    let label = ErrorLabel::from_error_chain(&chain(), |error| {
        error.downcast_ref::<Link>().and_then(|link| match link.value {
            1 => Some(ErrorLabel::from_static("outer")),
            3 => Some(ErrorLabel::from_static("root")),
            _ => None,
        })
    });
    assert_eq!(label.as_str(), "outer.root");
    assert_eq!(ErrorLabel::from_parts(std::iter::empty::<ErrorLabel>()).as_str(), "");
}

#[cfg(feature = "app-err")]
#[test]
fn gamma_survivor_app_error_traverses_all_sources_for_root_and_typed_search() {
    let error = ohno::AppError::new(chain());
    assert_eq!(error.root_cause().downcast_ref::<Link>().unwrap().value, 3);
    assert!(error.find_source::<std::io::Error>().is_none());
    assert_eq!(error.find_source::<Link>().unwrap().value, 1);
}
