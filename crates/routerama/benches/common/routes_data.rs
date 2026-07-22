// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared benchmark data: a realistic, GitHub-like route table, each capture
// tagged with the type routerama's `#[resolver]` coerces it to, plus the
// concrete request paths looked up. `include!`d by `harness.rs` and by
// `scripts/perf_report.rs` (which regenerates `bench_router.rs` from it), so
// it defines only data items.

/// How a captured variable is coerced, matching the field type routerama's
/// `#[resolver]` gives it. Competitors coerce each capture the same way so the
/// comparison measures equivalent work.
#[derive(Clone, Copy)]
enum Ty {
    /// `&str` — borrowed, zero-copy.
    Str,
    /// `u32` — parsed from the capture.
    U32,
    /// `String` — percent-decoded and owned.
    Owned,
}

/// `(name, path template, capture types in template order)`.
static ROUTES: &[(&str, &str, &[Ty])] = &[
    ("ListUsers", "/v1/users", &[]),
    ("GetUser", "/v1/users/{user}", &[Ty::Owned]),
    ("ListUserRepos", "/v1/users/{user}/repos", &[Ty::Owned]),
    ("ListUserFollowers", "/v1/users/{user}/followers", &[Ty::Owned]),
    ("ListUserFollowing", "/v1/users/{user}/following", &[Ty::Owned]),
    ("ListUserGists", "/v1/users/{user}/gists", &[Ty::Owned]),
    ("ListUserStarred", "/v1/users/{user}/starred", &[Ty::Owned]),
    ("ListUserEvents", "/v1/users/{user}/events", &[Ty::Owned]),
    ("ListUserReceivedEvents", "/v1/users/{user}/received_events", &[Ty::Owned]),
    ("GetRepo", "/v1/repos/{owner}/{repo}", &[Ty::Owned, Ty::Owned]),
    ("ListBranches", "/v1/repos/{owner}/{repo}/branches", &[Ty::Owned, Ty::Owned]),
    ("GetBranch", "/v1/repos/{owner}/{repo}/branches/{branch}", &[Ty::Owned, Ty::Owned, Ty::Str]),
    ("ListCommits", "/v1/repos/{owner}/{repo}/commits", &[Ty::Owned, Ty::Owned]),
    ("GetCommit", "/v1/repos/{owner}/{repo}/commits/{sha}", &[Ty::Owned, Ty::Owned, Ty::Str]),
    ("ListTags", "/v1/repos/{owner}/{repo}/tags", &[Ty::Owned, Ty::Owned]),
    ("ListLanguages", "/v1/repos/{owner}/{repo}/languages", &[Ty::Owned, Ty::Owned]),
    ("ListStargazers", "/v1/repos/{owner}/{repo}/stargazers", &[Ty::Owned, Ty::Owned]),
    ("ListSubscribers", "/v1/repos/{owner}/{repo}/subscribers", &[Ty::Owned, Ty::Owned]),
    ("ListIssues", "/v1/repos/{owner}/{repo}/issues", &[Ty::Owned, Ty::Owned]),
    ("GetIssue", "/v1/repos/{owner}/{repo}/issues/{issue}", &[Ty::Owned, Ty::Owned, Ty::U32]),
    ("ListIssueComments", "/v1/repos/{owner}/{repo}/issues/{issue}/comments", &[Ty::Owned, Ty::Owned, Ty::U32]),
    ("GetIssueComment", "/v1/repos/{owner}/{repo}/issues/{issue}/comments/{comment}", &[Ty::Owned, Ty::Owned, Ty::U32, Ty::U32]),
    ("ListIssueLabels", "/v1/repos/{owner}/{repo}/issues/{issue}/labels", &[Ty::Owned, Ty::Owned, Ty::U32]),
    ("ListPulls", "/v1/repos/{owner}/{repo}/pulls", &[Ty::Owned, Ty::Owned]),
    ("GetPull", "/v1/repos/{owner}/{repo}/pulls/{pull}", &[Ty::Owned, Ty::Owned, Ty::U32]),
    ("ListPullCommits", "/v1/repos/{owner}/{repo}/pulls/{pull}/commits", &[Ty::Owned, Ty::Owned, Ty::U32]),
    ("ListPullFiles", "/v1/repos/{owner}/{repo}/pulls/{pull}/files", &[Ty::Owned, Ty::Owned, Ty::U32]),
    ("MergePull", "/v1/repos/{owner}/{repo}/pulls/{pull}/merge", &[Ty::Owned, Ty::Owned, Ty::U32]),
    ("GetPullReview", "/v1/repos/{owner}/{repo}/pulls/{pull}/reviews/{review}", &[Ty::Owned, Ty::Owned, Ty::U32, Ty::U32]),
    ("ListReleases", "/v1/repos/{owner}/{repo}/releases", &[Ty::Owned, Ty::Owned]),
    ("GetRelease", "/v1/repos/{owner}/{repo}/releases/{release}", &[Ty::Owned, Ty::Owned, Ty::U32]),
    ("ListNotifications", "/v1/repos/{owner}/{repo}/notifications", &[Ty::Owned, Ty::Owned]),
    ("GetOrg", "/v1/orgs/{org}", &[Ty::Owned]),
    ("ListOrgMembers", "/v1/orgs/{org}/members", &[Ty::Owned]),
    ("GetOrgMember", "/v1/orgs/{org}/members/{user}", &[Ty::Owned, Ty::Owned]),
    ("ListOrgRepos", "/v1/orgs/{org}/repos", &[Ty::Owned]),
    ("ListOrgTeams", "/v1/orgs/{org}/teams", &[Ty::Owned]),
    ("GetOrgTeam", "/v1/orgs/{org}/teams/{team}", &[Ty::Owned, Ty::Str]),
    ("ListGists", "/v1/gists", &[]),
    ("GetGist", "/v1/gists/{gist}", &[Ty::Str]),
    ("ListGistComments", "/v1/gists/{gist}/comments", &[Ty::Str]),
    ("SearchRepositories", "/v1/search/repositories", &[]),
    ("SearchIssues", "/v1/search/issues", &[]),
    ("SearchUsers", "/v1/search/users", &[]),
    ("GetFeeds", "/v1/feeds", &[]),
    ("GetRateLimit", "/v1/rate_limit", &[]),
];

/// Concrete request paths exercised each lookup iteration; every one matches a
/// route, and every `U32` capture position is filled with a numeric value.
static LOOKUPS: &[&str] = &[
    "/v1/users",
    "/v1/users/octocat",
    "/v1/users/octocat/repos",
    "/v1/users/octocat/received_events",
    "/v1/repos/rust-lang/cargo",
    "/v1/repos/rust-lang/cargo/branches",
    "/v1/repos/rust-lang/cargo/branches/main",
    "/v1/repos/rust-lang/cargo/commits/2f8e1a9",
    "/v1/repos/rust-lang/cargo/issues",
    "/v1/repos/rust-lang/cargo/issues/1347",
    "/v1/repos/rust-lang/cargo/issues/1347/comments",
    "/v1/repos/rust-lang/cargo/issues/1347/comments/42",
    "/v1/repos/rust-lang/cargo/pulls/9001",
    "/v1/repos/rust-lang/cargo/pulls/9001/files",
    "/v1/repos/rust-lang/cargo/pulls/9001/merge",
    "/v1/repos/rust-lang/cargo/releases/7",
    "/v1/orgs/rust-lang",
    "/v1/orgs/rust-lang/members/octocat",
    "/v1/orgs/rust-lang/teams/core",
    "/v1/gists/aa11bb22/comments",
    "/v1/search/repositories",
    "/v1/feeds",
];
