// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Strict/relaxed parity checks across every documented interoperability class.

#![cfg(feature = "headers-all")]
#![expect(
    clippy::too_many_lines,
    reason = "one exhaustive matrix keeps every relaxed-decoding class together"
)]
#![expect(clippy::unwrap_used, reason = "test failures provide sufficient context")]

use http_headers::headers::{
    Accept, AcceptEncoding, AcceptLanguage, ContentRange, ContentType, ETag, Host, IfRange, IfRangeValueView, LastModified, Location, Range,
};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeMode, Field, FieldName};

#[derive(Debug, Eq, PartialEq)]
struct Projection {
    wire: Vec<Vec<u8>>,
    semantics: String,
}

struct Source {
    name: &'static FieldName,
    value: &'static [u8],
}

impl FieldSource for Source {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::single(name, self.value))
    }
}

type Project = fn(&Source) -> Projection;
type Reject = fn(&Source) -> bool;

struct Case {
    label: &'static str,
    name: &'static FieldName,
    relaxed: &'static [u8],
    invalid_neighbor: &'static [u8],
    strict_rejects: Reject,
    relaxed_rejects: Reject,
    borrowed: Project,
    owned: Project,
}

fn rejects_strict<H: Field>(source: &Source) -> bool {
    H::view(source).is_err() && H::owned(source).is_err()
}

fn rejects_relaxed<H: Field>(source: &Source) -> bool {
    H::view_with(source, DecodeMode::Relaxed).is_err() && H::owned_with(source, DecodeMode::Relaxed).is_err()
}

macro_rules! list_projection {
    ($borrowed:ident, $owned:ident, $header:ty) => {
        fn $borrowed(source: &Source) -> Projection {
            let value = <$header>::view_with(source, DecodeMode::Relaxed)
                .expect("relaxed borrowed decode")
                .expect("field is present");
            Projection {
                wire: value.values().map(|line| line.as_bytes().to_vec()).collect(),
                semantics: format!("{:?}", value.items().map(<[u8]>::to_vec).collect::<Vec<_>>()),
            }
        }

        fn $owned(source: &Source) -> Projection {
            let value = <$header>::owned_with(source, DecodeMode::Relaxed)
                .expect("relaxed owned decode")
                .expect("field is present");
            Projection {
                wire: value.values().map(|line| line.as_bytes().to_vec()).collect(),
                semantics: format!("{:?}", value.items().map(<[u8]>::to_vec).collect::<Vec<_>>()),
            }
        }
    };
}

list_projection!(accept_view, accept_owned, Accept);
list_projection!(accept_encoding_view, accept_encoding_owned, AcceptEncoding);
list_projection!(accept_language_view, accept_language_owned, AcceptLanguage);

fn etag_view(source: &Source) -> Projection {
    let value = ETag::view_with(source, DecodeMode::Relaxed)
        .expect("relaxed borrowed decode")
        .expect("field is present");
    Projection {
        wire: vec![source.value.to_vec()],
        semantics: format!("{}:{:?}", value.is_weak(), value.opaque_tag()),
    }
}

fn etag_owned(source: &Source) -> Projection {
    let value = ETag::owned_with(source, DecodeMode::Relaxed)
        .expect("relaxed owned decode")
        .expect("field is present");
    let semantics = format!("{}:{:?}", value.is_weak(), value.opaque_tag().expect("validated metadata"));
    Projection {
        wire: vec![value.into_field_value().as_bytes().to_vec()],
        semantics,
    }
}

fn content_type_view(source: &Source) -> Projection {
    let value = ContentType::view_with(source, DecodeMode::Relaxed)
        .expect("relaxed borrowed decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{}:{}", value.type_().unwrap(), value.subtype().unwrap()),
    }
}

fn content_type_owned(source: &Source) -> Projection {
    let value = ContentType::owned_with(source, DecodeMode::Relaxed)
        .expect("relaxed owned decode")
        .expect("field is present");
    let semantics = format!("{}:{}", value.type_().unwrap(), value.subtype().unwrap());
    Projection {
        wire: vec![value.into_field_value().as_bytes().to_vec()],
        semantics,
    }
}

fn range_view(source: &Source) -> Projection {
    let value = Range::view_with(source, DecodeMode::Relaxed)
        .expect("relaxed borrowed decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{:?}", value.byte_ranges().expect("byte ranges").collect::<Vec<_>>()),
    }
}

fn range_owned(source: &Source) -> Projection {
    let value = Range::owned_with(source, DecodeMode::Relaxed)
        .expect("relaxed owned decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{:?}", value.byte_ranges().expect("byte ranges").collect::<Vec<_>>()),
    }
}

fn content_range_view(source: &Source) -> Projection {
    let value = ContentRange::view_with(source, DecodeMode::Relaxed)
        .expect("relaxed borrowed decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{:?}", value.byte_range()),
    }
}

fn content_range_owned(source: &Source) -> Projection {
    let value = ContentRange::owned_with(source, DecodeMode::Relaxed)
        .expect("relaxed owned decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{:?}", value.byte_range()),
    }
}

fn last_modified_view(source: &Source) -> Projection {
    let value = LastModified::view_with(source, DecodeMode::Relaxed)
        .expect("relaxed borrowed decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{:?}", value.date()),
    }
}

fn last_modified_owned(source: &Source) -> Projection {
    let value = LastModified::owned_with(source, DecodeMode::Relaxed)
        .expect("relaxed owned decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{:?}", value.date()),
    }
}

fn if_range_view(source: &Source) -> Projection {
    let value = IfRange::view_with(source, DecodeMode::Relaxed)
        .expect("relaxed borrowed decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: if_range_semantics(value.value()),
    }
}

fn if_range_owned(source: &Source) -> Projection {
    let value = IfRange::owned_with(source, DecodeMode::Relaxed)
        .expect("relaxed owned decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: if_range_semantics(value.value().expect("validated metadata")),
    }
}

fn if_range_semantics(value: IfRangeValueView<'_>) -> String {
    match value {
        IfRangeValueView::EntityTag(tag) => format!("tag:{:?}", tag.opaque_tag()),
        IfRangeValueView::Date(date) => format!("date:{date:?}"),
    }
}

fn host_view(source: &Source) -> Projection {
    let value = Host::view_with(source, DecodeMode::Relaxed)
        .expect("relaxed borrowed decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{}:{:?}", value.host(), value.port()),
    }
}

fn host_owned(source: &Source) -> Projection {
    let value = Host::owned_with(source, DecodeMode::Relaxed)
        .expect("relaxed owned decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_field_value().as_bytes().to_vec()],
        semantics: format!("{}:{:?}", value.host().unwrap(), value.port().unwrap()),
    }
}

fn location_view(source: &Source) -> Projection {
    let value = Location::view_with(source, DecodeMode::Relaxed)
        .expect("relaxed borrowed decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_bytes().to_vec()],
        semantics: value.as_str().unwrap().to_owned(),
    }
}

fn location_owned(source: &Source) -> Projection {
    let value = Location::owned_with(source, DecodeMode::Relaxed)
        .expect("relaxed owned decode")
        .expect("field is present");
    Projection {
        wire: vec![value.as_bytes().to_vec()],
        semantics: value.as_str().unwrap().to_owned(),
    }
}

#[test]
fn strict_and_relaxed_borrowed_owned_semantics_stay_in_parity() {
    let cases = [
        Case {
            label: "Accept qvalue precision",
            name: &FieldName::Accept,
            relaxed: b"text/html; q = .12345",
            invalid_neighbor: b"text/html;q=-.5",
            strict_rejects: rejects_strict::<Accept>,
            relaxed_rejects: rejects_relaxed::<Accept>,
            borrowed: accept_view,
            owned: accept_owned,
        },
        Case {
            label: "Accept-Encoding qvalue whitespace",
            name: &FieldName::AcceptEncoding,
            relaxed: b"gzip; q=.5",
            invalid_neighbor: b"gzip;q=1.1",
            strict_rejects: rejects_strict::<AcceptEncoding>,
            relaxed_rejects: rejects_relaxed::<AcceptEncoding>,
            borrowed: accept_encoding_view,
            owned: accept_encoding_owned,
        },
        Case {
            label: "Accept-Language qvalue precision",
            name: &FieldName::AcceptLanguage,
            relaxed: b"en-US; Q = 1.0000",
            invalid_neighbor: b"en_US;q=.5",
            strict_rejects: rejects_strict::<AcceptLanguage>,
            relaxed_rejects: rejects_relaxed::<AcceptLanguage>,
            borrowed: accept_language_view,
            owned: accept_language_owned,
        },
        Case {
            label: "lowercase weak ETag",
            name: &FieldName::Etag,
            relaxed: b"w/\"revision\"",
            invalid_neighbor: b"w/\"bad tag\"",
            strict_rejects: rejects_strict::<ETag>,
            relaxed_rejects: rejects_relaxed::<ETag>,
            borrowed: etag_view,
            owned: etag_owned,
        },
        Case {
            label: "Content-Type delimiter whitespace",
            name: &FieldName::ContentType,
            relaxed: b"text / html",
            invalid_neighbor: b"text // html",
            strict_rejects: rejects_strict::<ContentType>,
            relaxed_rejects: rejects_relaxed::<ContentType>,
            borrowed: content_type_view,
            owned: content_type_owned,
        },
        Case {
            label: "Range delimiter whitespace",
            name: &FieldName::Range,
            relaxed: b"bytes = 0 - 499",
            invalid_neighbor: b"bytes = +1 - 2",
            strict_rejects: rejects_strict::<Range>,
            relaxed_rejects: rejects_relaxed::<Range>,
            borrowed: range_view,
            owned: range_owned,
        },
        Case {
            label: "Content-Range delimiter whitespace",
            name: &FieldName::ContentRange,
            relaxed: b"bytes 0 - 499 / 1234",
            invalid_neighbor: b"bytes  0 - 1 / 2",
            strict_rejects: rejects_strict::<ContentRange>,
            relaxed_rejects: rejects_relaxed::<ContentRange>,
            borrowed: content_range_view,
            owned: content_range_owned,
        },
        Case {
            label: "relaxed IMF-style date",
            name: &FieldName::LastModified,
            relaxed: b" Sun, 6 Nov 1994 8:49:37 UTC ",
            invalid_neighbor: b"Sun,\t6 Nov 1994 8:49:37 UTC",
            strict_rejects: rejects_strict::<LastModified>,
            relaxed_rejects: rejects_relaxed::<LastModified>,
            borrowed: last_modified_view,
            owned: last_modified_owned,
        },
        Case {
            label: "relaxed If-Range date",
            name: &FieldName::IfRange,
            relaxed: b" Tue, 8 Nov 1994 8:49:37 UTC ",
            invalid_neighbor: b"Tue,\t8 Nov 1994 8:49:37 UTC",
            strict_rejects: rejects_strict::<IfRange>,
            relaxed_rejects: rejects_relaxed::<IfRange>,
            borrowed: if_range_view,
            owned: if_range_owned,
        },
        Case {
            label: "internationalized Host",
            name: &FieldName::Host,
            relaxed: "münich.example:443".as_bytes(),
            invalid_neighbor: "münich@example".as_bytes(),
            strict_rejects: rejects_strict::<Host>,
            relaxed_rejects: rejects_relaxed::<Host>,
            borrowed: host_view,
            owned: host_owned,
        },
        Case {
            label: "Location backslashes",
            name: &FieldName::Location,
            relaxed: br"/a\b\c",
            invalid_neighbor: br"bad\%zz",
            strict_rejects: rejects_strict::<Location>,
            relaxed_rejects: rejects_relaxed::<Location>,
            borrowed: location_view,
            owned: location_owned,
        },
    ];

    for case in cases {
        let source = Source {
            name: case.name,
            value: case.relaxed,
        };
        assert!((case.strict_rejects)(&source), "{} must remain strict by default", case.label);
        let borrowed = (case.borrowed)(&source);
        let owned = (case.owned)(&source);
        assert_eq!(borrowed, owned, "{} borrowed/owned parity", case.label);
        assert_eq!(borrowed.wire, [case.relaxed.to_vec()], "{} preserves wire bytes", case.label);

        let invalid = Source {
            name: case.name,
            value: case.invalid_neighbor,
        };
        assert!((case.relaxed_rejects)(&invalid), "{} invalid neighbor", case.label);
    }
}
