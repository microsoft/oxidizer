// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared benchmark data: a realistic, GitHub-like route table plus the concrete
// request paths looked up during the "compare routers" benchmark. Every route
// uses only literal segments and single-segment `{var}` parameters — the common
// subset every router being compared can express — so all routers resolve an
// identical set of routes and the lookup comparison is apples-to-apples.
//
// This file is `include!`d (by `common/harness.rs` and by the offline generator
// that produces `common/generated_router.rs`), so it defines only data items.

/// `(name, path template)` routes, keyed by a caller-chosen route name. Templates
/// use `{var}` for a single dynamic segment, as in the `google.api.http` grammar.
static ROUTES: &[(&str, &str)] = &[
    ("ListUsers", "/v1/users"),
    ("GetUser", "/v1/users/{user}"),
    ("ListUserRepos", "/v1/users/{user}/repos"),
    ("ListUserFollowers", "/v1/users/{user}/followers"),
    ("ListUserFollowing", "/v1/users/{user}/following"),
    ("ListUserGists", "/v1/users/{user}/gists"),
    ("ListUserStarred", "/v1/users/{user}/starred"),
    ("ListUserEvents", "/v1/users/{user}/events"),
    ("ListUserReceivedEvents", "/v1/users/{user}/received_events"),
    ("GetRepo", "/v1/repos/{owner}/{repo}"),
    ("ListBranches", "/v1/repos/{owner}/{repo}/branches"),
    ("GetBranch", "/v1/repos/{owner}/{repo}/branches/{branch}"),
    ("ListCommits", "/v1/repos/{owner}/{repo}/commits"),
    ("GetCommit", "/v1/repos/{owner}/{repo}/commits/{sha}"),
    ("ListTags", "/v1/repos/{owner}/{repo}/tags"),
    ("ListLanguages", "/v1/repos/{owner}/{repo}/languages"),
    ("ListStargazers", "/v1/repos/{owner}/{repo}/stargazers"),
    ("ListSubscribers", "/v1/repos/{owner}/{repo}/subscribers"),
    ("ListIssues", "/v1/repos/{owner}/{repo}/issues"),
    ("GetIssue", "/v1/repos/{owner}/{repo}/issues/{issue}"),
    ("ListIssueComments", "/v1/repos/{owner}/{repo}/issues/{issue}/comments"),
    ("GetIssueComment", "/v1/repos/{owner}/{repo}/issues/{issue}/comments/{comment}"),
    ("ListIssueLabels", "/v1/repos/{owner}/{repo}/issues/{issue}/labels"),
    ("ListPulls", "/v1/repos/{owner}/{repo}/pulls"),
    ("GetPull", "/v1/repos/{owner}/{repo}/pulls/{pull}"),
    ("ListPullCommits", "/v1/repos/{owner}/{repo}/pulls/{pull}/commits"),
    ("ListPullFiles", "/v1/repos/{owner}/{repo}/pulls/{pull}/files"),
    ("MergePull", "/v1/repos/{owner}/{repo}/pulls/{pull}/merge"),
    ("GetPullReview", "/v1/repos/{owner}/{repo}/pulls/{pull}/reviews/{review}"),
    ("ListReleases", "/v1/repos/{owner}/{repo}/releases"),
    ("GetRelease", "/v1/repos/{owner}/{repo}/releases/{release}"),
    ("ListNotifications", "/v1/repos/{owner}/{repo}/notifications"),
    ("GetOrg", "/v1/orgs/{org}"),
    ("ListOrgMembers", "/v1/orgs/{org}/members"),
    ("GetOrgMember", "/v1/orgs/{org}/members/{user}"),
    ("ListOrgRepos", "/v1/orgs/{org}/repos"),
    ("ListOrgTeams", "/v1/orgs/{org}/teams"),
    ("GetOrgTeam", "/v1/orgs/{org}/teams/{team}"),
    ("ListGists", "/v1/gists"),
    ("GetGist", "/v1/gists/{gist}"),
    ("ListGistComments", "/v1/gists/{gist}/comments"),
    ("SearchRepositories", "/v1/search/repositories"),
    ("SearchIssues", "/v1/search/issues"),
    ("SearchUsers", "/v1/search/users"),
    ("GetFeeds", "/v1/feeds"),
    ("GetRateLimit", "/v1/rate_limit"),
];

/// Concrete request paths exercised on every lookup iteration: a mix of shallow
/// and deep hits spread across the route table (parameters filled with
/// representative values). All of these match a route.
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
