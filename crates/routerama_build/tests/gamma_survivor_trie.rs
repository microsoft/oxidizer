// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Trie metadata, capture plans, and conflict regression oracles.

use http_path_template::{Grammar, PathTemplate};
use routerama_build::Route;
use routerama_build::trie::{VarPlan, affix_edges_in_match_order, build_trie, capture_field_names, conflicts, route_field_name};

#[cfg(test)]
fn route(name: &str, method: &str, path: &str) -> Route {
    Route::new(
        name,
        method,
        PathTemplate::parse(path, Grammar::default().with_segment_affixes()).unwrap(),
    )
}

#[test]
fn gamma_survivor_trie_capture_plans_and_metadata() {
    let routes = [
        route("Span", "GET", "/{book.id=shelves/*}/pre-{type}.json/{tail=**}:archive"),
        route("Short", "POST", "/short"),
    ];
    assert_eq!(routes[0].name(), "Span");
    assert_eq!(routes[0].method(), "GET");
    assert_eq!(
        routes[0].template().to_string(),
        "/{book.id=shelves/*}/pre-{type}.json/{tail=**}:archive"
    );
    assert_eq!(
        capture_field_names(routes[0].template().segments()),
        [
            vec!["book".to_owned(), "id".to_owned()],
            vec!["type".to_owned()],
            vec!["tail".to_owned()]
        ]
    );
    let trie = build_trie(&routes);
    assert_eq!(trie.max_segments, 4);
    assert!(trie.any_verb);
    let leaf = &trie.root.literals["shelves"].single.as_ref().unwrap().affix[&("pre-".into(), ".json".into())].rest[0];
    assert_eq!(
        (leaf.name.as_str(), leaf.method.as_str(), leaf.verb.as_deref(), leaf.route_index),
        ("Span", "GET", Some("archive"), 0)
    );
    assert_eq!(leaf.vars.len(), 3);
    assert!(matches!(&leaf.vars[0], VarPlan::Span { a: 0, b: 1, .. }));
    assert!(matches!(
        &leaf.vars[1],
        VarPlan::Affix {
            a: 2,
            prefix_len: 4,
            suffix_len: 5,
            ..
        }
    ));
    assert!(matches!(&leaf.vars[2], VarPlan::Rest { a: 3, .. }));
    assert_eq!(
        leaf.vars.iter().map(|plan| (plan.field(), plan.key())).collect::<Vec<_>>(),
        [("book_id", "book.id"), ("_f_type", "type"), ("tail", "tail")]
    );
    assert!(conflicts(&trie.root).is_empty());
    let empty = build_trie(&[]);
    assert_eq!(empty.max_segments, 0);
    assert!(!empty.any_verb);
}

#[test]
fn gamma_survivor_trie_conflicts_are_bucket_specific() {
    let routes = [
        route("A", "GET", "/"),
        route("B", "GET", "/"),
        route("C", "POST", "/"),
        route("D", "GET", "/x/{id}"),
        route("E", "GET", "/x/{other}"),
        route("F", "GET", "/x/pre-{id}.txt"),
        route("G", "GET", "/x/pre-{other}.txt"),
        route("H", "POST", "/x/{rest=**}:go"),
        route("I", "POST", "/x/**:go"),
        route("J", "POST", "/x/**:other"),
    ];
    assert_eq!(
        conflicts(&build_trie(&routes).root),
        [
            "conflicting routes: `GET /` maps to multiple names (A, B); each HTTP method and path may resolve to only one route",
            "conflicting routes: `POST /x/**:go` maps to multiple names (H, I); each HTTP method and path may resolve to only one route",
            "conflicting routes: `GET /x/pre-{}.txt` maps to multiple names (F, G); each HTTP method and path may resolve to only one route",
            "conflicting routes: `GET /x/*` maps to multiple names (D, E); each HTTP method and path may resolve to only one route",
        ]
    );
}

#[test]
fn gamma_survivor_affix_specificity_and_field_sanitizing() {
    let trie = build_trie(&[
        route("A", "GET", "/a-{id}.txt"),
        route("B", "GET", "/abc-{id}"),
        route("C", "GET", "/{id}.txt"),
        route("D", "GET", "/z-{id}.txt"),
    ]);
    assert_eq!(
        affix_edges_in_match_order(&trie.root)
            .into_iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>(),
        [
            ("a-".into(), ".txt".into()),
            ("z-".into(), ".txt".into()),
            (String::new(), ".txt".into()),
            ("abc-".into(), String::new())
        ]
    );
    for (input, output) in [
        ("book.id", "book_id"),
        ("type", "_f_type"),
        ("self", "_f_self"),
        ("normal", "normal"),
        ("9foo", "_f_9foo"),
        ("a-b", "_f_a_b"),
    ] {
        assert_eq!(route_field_name(input), output);
    }
}
