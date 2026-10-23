// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adversarial request-path coverage for generated and runtime resolution.

use std::fmt::Write as _;
use std::process::Command;

use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route};
use routerama::{HttpMethod, ResolveError, resolver};

#[resolver]
#[derive(Debug)]
enum AdversarialRoute<'p> {
    #[route(GET, "/health")]
    Health,
    #[route(GET, "/raw/{value}")]
    Raw {
        value: &'p str,
    },
    #[route(GET, "/owned/{value}")]
    Owned {
        value: String,
    },
    #[route(GET, "/files/{tail=**}")]
    Files {
        tail: &'p str,
    },
    #[route(POST, "/jobs/{id}:cancel")]
    Cancel {
        id: &'p str,
    },
    Dynamic {
        value: String,
    },
}

fn resolver() -> AdversarialRouteResolver {
    AdversarialRoute::builder()
        .add_dynamic(HttpMethod::GET, "/dynamic/{value}")
        .build()
        .expect("adversarial resolver builds")
}

#[test]
#[cfg_attr(miri, ignore)]
fn attacker_controlled_paths_remain_bounded_and_non_recursive() {
    const CHILD_ENV: &str = "ROUTERAMA_ADVERSARIAL_PATH_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = Command::new(std::env::current_exe().expect("current test executable is available"))
            .args([
                "--exact",
                "attacker_controlled_paths_remain_bounded_and_non_recursive",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .expect("adversarial-path child starts");
        assert!(
            output.status.success(),
            "adversarial-path child failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let resolver = resolver();

    let long_segment = "x".repeat(64 * 1024);
    assert!(matches!(resolver.resolve("GET", &long_segment), Err(ResolveError::NotFound(_))));

    let many_segments = "/x".repeat(32 * 1024);
    assert!(matches!(resolver.resolve("GET", &many_segments), Err(ResolveError::NotFound(_))));

    let empty_segments = "/".repeat(64 * 1024);
    assert!(matches!(resolver.resolve("GET", &empty_segments), Err(ResolveError::NotFound(_))));

    let capture = "z".repeat(64 * 1024);
    let raw_path = format!("/raw/{capture}");
    match resolver.resolve("GET", &raw_path) {
        Ok(AdversarialRoute::Raw { value }) => assert_eq!(value.len(), capture.len()),
        other => panic!("large borrowed capture did not resolve: {other:?}"),
    }

    let owned_path = format!("/owned/{capture}");
    match resolver.resolve("GET", &owned_path) {
        Ok(AdversarialRoute::Owned { value }) => assert_eq!(value, capture),
        other => panic!("large owned capture did not resolve: {other:?}"),
    }

    let dynamic_path = format!("/dynamic/{capture}");
    match resolver.resolve("GET", &dynamic_path) {
        Ok(AdversarialRoute::Dynamic { value }) => assert_eq!(value, capture),
        other => panic!("large dynamic capture did not resolve: {other:?}"),
    }

    let rest = "segment/".repeat(8 * 1024);
    let rest_path = format!("/files/{rest}");
    match resolver.resolve("GET", &rest_path) {
        Ok(AdversarialRoute::Files { tail }) => assert_eq!(tail, rest),
        other => panic!("large rest capture did not resolve: {other:?}"),
    }

    for malformed in ["/owned/%", "/owned/%0", "/owned/%GG", "/owned/%FF"] {
        assert!(
            matches!(resolver.resolve("GET", malformed), Err(ResolveError::UndecodableCapture(_))),
            "malformed escape unexpectedly resolved: {malformed}"
        );
    }

    match resolver.resolve("GET", "/raw/東京/🦀") {
        Err(ResolveError::NotFound(_)) => {}
        other => panic!("an extra Unicode segment must not be truncated: {other:?}"),
    }
    match resolver.resolve("GET", "/raw/東京🦀") {
        Ok(AdversarialRoute::Raw { value }) => assert_eq!(value, "東京🦀"),
        other => panic!("Unicode capture did not preserve boundaries: {other:?}"),
    }

    match resolver.resolve("POST", "/jobs/a:b:cancel") {
        Ok(AdversarialRoute::Cancel { id }) => assert_eq!(id, "a:b"),
        other => panic!("final custom verb was not selected: {other:?}"),
    }

    let mut template = String::new();
    let mut path = String::new();
    for index in 0..64 {
        let _ = write!(template, "/{{capture{index}}}");
        let _ = write!(path, "/value{index}");
    }
    let raw = RawResolver::new([Route::new(
        "ManyCaptures",
        "GET",
        PathTemplate::parse(&template, Grammar::default()).expect("high-capture template is valid"),
    )]);
    assert!(raw.resolve("GET", &many_segments).is_none());
    let matched = raw.resolve("GET", &path).expect("high-capture path resolves");
    assert_eq!(matched.captures().count(), 64);
    assert_eq!(routerama::__rt::RouteMatch::capture(&matched, "capture63"), Some("value63"));
}
