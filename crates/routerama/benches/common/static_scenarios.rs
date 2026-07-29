// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared focused static-router scenarios for Criterion and Callgrind.

use std::hint::black_box;

#[::routerama::resolver]
#[derive(Debug)]
enum StaticScenario<'p> {
    #[route(GET, "/health")]
    Health,
    #[route(GET, "/a/b/c/d/e/f/g/h")]
    Deep,
    #[route(GET, "/fanout/00")]
    Fanout00,
    #[route(GET, "/fanout/01")]
    Fanout01,
    #[route(GET, "/fanout/02")]
    Fanout02,
    #[route(GET, "/fanout/03")]
    Fanout03,
    #[route(GET, "/fanout/04")]
    Fanout04,
    #[route(GET, "/fanout/05")]
    Fanout05,
    #[route(GET, "/fanout/06")]
    Fanout06,
    #[route(GET, "/fanout/07")]
    Fanout07,
    #[route(GET, "/fanout/08")]
    Fanout08,
    #[route(GET, "/fanout/09")]
    Fanout09,
    #[route(GET, "/fanout/10")]
    Fanout10,
    #[route(GET, "/fanout/11")]
    Fanout11,
    #[route(GET, "/fanout/12")]
    Fanout12,
    #[route(GET, "/fanout/13")]
    Fanout13,
    #[route(GET, "/fanout/14")]
    Fanout14,
    #[route(GET, "/fanout/15")]
    Fanout15,
    #[route(GET, "/users/{user}")]
    BorrowOne { user: &'p str },
    #[route(GET, "/orgs/{org}/repos/{repo}/refs/{kind}/{name}")]
    BorrowFour {
        org: &'p str,
        repo: &'p str,
        kind: &'p str,
        name: &'p str,
    },
    #[route(GET, "/issues/{issue}")]
    ParseNumber { issue: u32 },
    #[route(GET, "/owned/{name}")]
    OwnString { name: String },
    #[route(POST, "/submit")]
    Submit,
    #[route(GET, "/files/{path=**}")]
    Files { path: &'p str },
    #[route(GET, "/img-{id}.png")]
    Image { id: &'p str },
}

#[::routerama::resolver]
#[derive(Debug)]
enum NoVerbScenario<'p> {
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
}

#[::routerama::resolver]
#[derive(Debug)]
enum WithVerbScenario<'p> {
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/books/{book}:archive")]
    ArchiveBook { book: &'p str },
}

#[::routerama::resolver]
#[derive(Debug)]
enum ShallowTable {
    #[route(GET, "/hot")]
    Hot,
}

#[::routerama::resolver]
#[derive(Debug)]
enum DeepOutlierTable {
    #[route(GET, "/hot")]
    Hot,
    #[route(
        GET,
        "/deep/01/02/03/04/05/06/07/08/09/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31"
    )]
    Deep,
}

#[::routerama::resolver]
#[derive(Debug)]
enum AffixFanout<'p> {
    #[route(GET, "/asset-00-{id}.png")]
    Asset00 { id: &'p str },
    #[route(GET, "/asset-01-{id}.png")]
    Asset01 { id: &'p str },
    #[route(GET, "/asset-02-{id}.png")]
    Asset02 { id: &'p str },
    #[route(GET, "/asset-03-{id}.png")]
    Asset03 { id: &'p str },
    #[route(GET, "/asset-04-{id}.png")]
    Asset04 { id: &'p str },
    #[route(GET, "/asset-05-{id}.png")]
    Asset05 { id: &'p str },
    #[route(GET, "/asset-06-{id}.png")]
    Asset06 { id: &'p str },
    #[route(GET, "/asset-07-{id}.png")]
    Asset07 { id: &'p str },
    #[route(GET, "/asset-08-{id}.png")]
    Asset08 { id: &'p str },
    #[route(GET, "/asset-09-{id}.png")]
    Asset09 { id: &'p str },
    #[route(GET, "/asset-10-{id}.png")]
    Asset10 { id: &'p str },
    #[route(GET, "/asset-11-{id}.png")]
    Asset11 { id: &'p str },
    #[route(GET, "/asset-12-{id}.png")]
    Asset12 { id: &'p str },
    #[route(GET, "/asset-13-{id}.png")]
    Asset13 { id: &'p str },
    #[route(GET, "/asset-14-{id}.png")]
    Asset14 { id: &'p str },
    #[route(GET, "/asset-15-{id}.png")]
    Asset15 { id: &'p str },
}

type StaticScenarioRouter = StaticScenarioResolver;
type NoVerbScenarioRouter = NoVerbScenarioResolver;
type WithVerbScenarioRouter = WithVerbScenarioResolver;
type ShallowTableRouter = ShallowTableResolver;
type DeepOutlierTableRouter = DeepOutlierTableResolver;
type AffixFanoutRouter = AffixFanoutResolver;

fn build_static_scenario() -> StaticScenarioRouter {
    StaticScenario::resolver()
}

fn build_no_verb_scenario() -> NoVerbScenarioRouter {
    NoVerbScenario::resolver()
}

fn build_with_verb_scenario() -> WithVerbScenarioRouter {
    WithVerbScenario::resolver()
}

fn build_shallow_table() -> ShallowTableRouter {
    ShallowTable::resolver()
}

fn build_deep_outlier_table() -> DeepOutlierTableRouter {
    DeepOutlierTable::resolver()
}

fn build_affix_fanout() -> AffixFanoutRouter {
    AffixFanout::resolver()
}

#[inline]
fn static_shallow_literal(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/health")));
}

#[inline]
fn static_deep_literal(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/a/b/c/d/e/f/g/h")));
}

#[inline]
fn static_fanout_first(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/fanout/00")));
}

#[inline]
fn static_fanout_middle(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/fanout/08")));
}

#[inline]
fn static_fanout_last(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/fanout/15")));
}

#[inline]
fn static_borrow_one(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/users/alice")));
}

#[inline]
fn static_borrow_four(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/orgs/acme/repos/api/refs/heads/main")));
}

#[inline]
fn static_parse_number(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/issues/12345")));
}

#[inline]
fn static_own_plain(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/owned/rust")));
}

#[inline]
fn static_own_percent(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/owned/%72ust")));
}

#[inline]
fn static_early_miss(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/missing")));
}

#[inline]
fn static_late_miss(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/a/b/c/d/e/f/g/missing")));
}

#[inline]
fn static_pathological_long_miss(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve(
        "GET",
        black_box(
            "/00/01/02/03/04/05/06/07/08/09/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31/32/33/34/35/36/37/38/39/40/41/42/43/44/45/46/47/48/49/50/51/52/53/54/55/56/57/58/59/60/61/62/63",
        ),
    ));
}

#[inline]
fn static_wrong_method(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/submit")));
}

#[inline]
fn static_rest(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/files/a/b/c")));
}

#[inline]
fn static_affix(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/img-cat.png")));
}

#[inline]
fn static_no_verb(router: &NoVerbScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/books/rust")));
}

#[inline]
fn static_with_verb_nonverb_hit(router: &WithVerbScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/books/rust")));
}

#[inline]
fn static_with_verb_hit(router: &WithVerbScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/books/rust:archive")));
}

#[inline]
fn static_shallow_table_hit(router: &ShallowTableRouter) {
    let _ = black_box(router.resolve("GET", black_box("/hot")));
}

#[inline]
fn static_deep_outlier_table_hit(router: &DeepOutlierTableRouter) {
    let _ = black_box(router.resolve("GET", black_box("/hot")));
}

#[inline]
fn static_affix_fanout_first(router: &AffixFanoutRouter) {
    let _ = black_box(router.resolve("GET", black_box("/asset-00-cat.png")));
}

#[inline]
fn static_affix_fanout_middle(router: &AffixFanoutRouter) {
    let _ = black_box(router.resolve("GET", black_box("/asset-08-cat.png")));
}

#[inline]
fn static_affix_fanout_last(router: &AffixFanoutRouter) {
    let _ = black_box(router.resolve("GET", black_box("/asset-15-cat.png")));
}
