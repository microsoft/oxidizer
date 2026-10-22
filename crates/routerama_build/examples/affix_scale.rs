//! Scaling probe for the affix sibling conflict check.
use std::time::Instant;

use http_path_template::{Grammar, PathTemplate};
use routerama_build::Route;
use routerama_build::trie::{build_trie, conflicts};

/// Siblings sharing one specificity with distinct prefix lengths, but prefixes
/// that are not prefixes of one another, so no pair actually conflicts.
fn measure(n: usize) -> (u128, usize) {
    let patterns: Vec<String> = (1..=n)
        .map(|i| {
            // A unique four-hex-digit lead makes no prefix a prefix of another.
            let prefix = format!("{i:04x}{}", "a".repeat(i));
            let suffix = "b".repeat(n - i + 1);
            format!("/x/{prefix}{{v}}{suffix}")
        })
        .collect();
    let routes: Vec<Route> = patterns
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let t = PathTemplate::parse(p, Grammar::default().with_segment_affixes()).expect("valid template");
            Route::new(format!("R{i}"), "GET", t)
        })
        .collect();
    let trie = build_trie(&routes);
    let start = Instant::now();
    let found = conflicts(&trie.root);
    (start.elapsed().as_micros(), found.len())
}

fn main() {
    for n in [64usize, 128, 256, 512, 1024] {
        let (us, found) = measure(n);
        println!("n={n:5}  {us:>9} us  conflicts={found}");
    }
}
