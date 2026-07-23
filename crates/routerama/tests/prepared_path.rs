// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `PreparedPath`: preparation feeds the resolver without changing how it matches.

use routerama::path::{DotSegments, EncodedSeparators, PathError, PathPolicy, PreparedPath, RepeatedSlashes, TrailingSlash};
use routerama::resolve::resolver;

#[resolver]
#[derive(Debug, PartialEq)]
enum Route<'p> {
    #[route(GET, "/public/{name}")]
    Public { name: &'p str },

    #[route(GET, "/admin/{name}")]
    Admin { name: &'p str },

    #[route(GET, "/files/{path=**}")]
    File { path: &'p str },
}

/// The spellings a fronting proxy would fold together but the resolver would not.
const DESYNCED: &[(&str, &str)] = &[
    ("/admin/../public/secret", "/public/secret"),
    ("//admin/root", "/admin/root"),
    ("/admin/root/", "/admin/root"),
    ("/./admin/root", "/admin/root"),
    ("/public//./items/../report", "/public/report"),
];

#[test]
fn a_prepared_path_resolves_exactly_as_its_canonical_spelling_does() {
    for (raw, canonical) in DESYNCED {
        let prepared = PreparedPath::new(raw, PathPolicy::STRICT).expect("every spelling here is canonicalizable");
        assert_eq!(prepared.as_str(), *canonical, "{raw}");
        assert!(prepared.was_changed(), "{raw}");

        let via_preparation = Route::resolver().resolve("GET", prepared.as_str());
        let direct = Route::resolver().resolve("GET", canonical);
        assert_eq!(via_preparation, direct, "{raw}");
    }
}

#[test]
fn preparation_is_what_makes_the_desynced_spellings_agree() {
    // Without preparation the resolver sees the bytes as sent, so a proxy that
    // authorized `/public/secret` would be forwarding a request this table does
    // not agree is that route at all. Preparation is what makes the two views
    // agree.
    Route::resolver().resolve("GET", "/admin/../public/secret").unwrap_err();
    assert_eq!(Route::resolver().resolve("GET", "/admin/.."), Ok(Route::Admin { name: ".." }));

    let prepared = PreparedPath::new("/admin/../public/secret", PathPolicy::STRICT).expect("canonicalizable");
    assert_eq!(
        Route::resolver().resolve("GET", prepared.as_str()),
        Ok(Route::Public { name: "secret" })
    );
}

#[test]
fn the_exact_policy_leaves_resolution_byte_for_byte_unchanged() {
    for (raw, _) in DESYNCED {
        let prepared = PreparedPath::new(raw, PathPolicy::EXACT).expect("the exact policy rejects only non-paths");
        assert_eq!(prepared.as_str(), *raw);
        assert_eq!(
            Route::resolver().resolve("GET", prepared.as_str()),
            Route::resolver().resolve("GET", raw),
            "{raw}"
        );
    }
}

#[test]
fn a_prepared_path_outlives_the_captures_borrowed_out_of_it() {
    let prepared = PreparedPath::new("/files//reports/../q1.csv", PathPolicy::STRICT).expect("canonicalizable");
    let resolved = Route::resolver().resolve("GET", prepared.as_str()).expect("matches the catch-all");
    assert_eq!(resolved, Route::File { path: "q1.csv" });
}

#[test]
fn the_strict_policy_refuses_an_encoded_separator_rather_than_decoding_it() {
    // Decoding would manufacture structure the sender never sent, so the only
    // safe answers are to keep it encoded or to refuse the request.
    assert_eq!(
        PreparedPath::new("/admin%2F..%2Fpublic", PathPolicy::STRICT),
        Err(PathError::EncodedSeparator)
    );
    assert_eq!(
        PreparedPath::new("/admin/%2e%2e/public", PathPolicy::STRICT),
        Err(PathError::EncodedSeparator)
    );

    let preserved = PreparedPath::new("/files/a%2Fb", PathPolicy::EXACT).expect("preserved");
    assert_eq!(
        Route::resolver().resolve("GET", preserved.as_str()),
        Ok(Route::File { path: "a%2Fb" })
    );
}

#[test]
fn removing_dot_segments_alone_does_not_neutralize_an_encoded_dot_segment() {
    // Nothing is decoded before matching, so `%2e%2e` stays an opaque segment
    // no matter what the dot-segment axis says. Rejecting encoded separators is
    // what covers it.
    let policy = PathPolicy::EXACT.with_dot_segments(DotSegments::Remove);
    let prepared = PreparedPath::new("/admin/%2e%2e", policy).expect("nothing here is a literal dot segment");
    assert_eq!(prepared.as_str(), "/admin/%2e%2e");
    assert!(!prepared.was_changed());
    assert_eq!(
        Route::resolver().resolve("GET", prepared.as_str()),
        Ok(Route::Admin { name: "%2e%2e" })
    );
}

#[test]
fn a_rejecting_policy_turns_a_non_canonical_spelling_into_a_refusal() {
    let policy = PathPolicy::EXACT
        .with_repeated_slashes(RepeatedSlashes::Reject)
        .with_dot_segments(DotSegments::Reject)
        .with_trailing_slash(TrailingSlash::Reject)
        .with_encoded_separators(EncodedSeparators::Reject);

    assert_eq!(PreparedPath::new("//admin/root", policy), Err(PathError::RepeatedSlashes));
    assert_eq!(PreparedPath::new("/admin/../public", policy), Err(PathError::DotSegment));
    assert_eq!(PreparedPath::new("/admin/root/", policy), Err(PathError::TrailingSlash));
    assert_eq!(PreparedPath::new("/admin/%2F", policy), Err(PathError::EncodedSeparator));

    let accepted = PreparedPath::new("/admin/root", policy).expect("already canonical");
    assert!(!accepted.was_changed());
    assert_eq!(
        Route::resolver().resolve("GET", accepted.as_str()),
        Ok(Route::Admin { name: "root" })
    );
}
