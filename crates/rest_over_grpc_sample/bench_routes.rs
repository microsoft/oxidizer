// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// A large, realistic route table shared between `build.rs` (which lowers it
// into the build-time generated static router) and the `router_vs_axum`
// benchmark (which builds equivalent `axum` and `matchit` routers from it).
//
// Each entry is `(rpc, method, google.api.http path template)`. The set models
// a GitHub-like API: nested resources, multiple methods per path, and a couple
// of `**` catch-alls. It deliberately stays within the routing features that
// `axum`/`matchit` can also express (literals, single-segment `{var}`, and a
// trailing `{var=**}` catch-all) so the three routers resolve an identical set
// of routes.
//
// This file is `include!`d, so it intentionally defines only a single item.

/// `(rpc, method, path template)` triples describing the benchmarked service's
/// routes, shared by the build script and the `grs_router_vs_axum` benchmark.
pub static ROUTES: &[(&str, &str, &str)] = &[
    // Users.
    ("ListUsers", "GET", "/v1/users"),
    ("CreateUser", "POST", "/v1/users"),
    ("GetUser", "GET", "/v1/users/{user}"),
    ("UpdateUser", "PATCH", "/v1/users/{user}"),
    ("DeleteUser", "DELETE", "/v1/users/{user}"),
    ("ListUserRepos", "GET", "/v1/users/{user}/repos"),
    ("ListUserFollowers", "GET", "/v1/users/{user}/followers"),
    ("ListUserFollowing", "GET", "/v1/users/{user}/following"),
    ("ListUserGists", "GET", "/v1/users/{user}/gists"),
    ("ListUserStarred", "GET", "/v1/users/{user}/starred"),
    // Repositories.
    ("GetRepo", "GET", "/v1/repos/{owner}/{repo}"),
    ("UpdateRepo", "PATCH", "/v1/repos/{owner}/{repo}"),
    ("DeleteRepo", "DELETE", "/v1/repos/{owner}/{repo}"),
    ("ListBranches", "GET", "/v1/repos/{owner}/{repo}/branches"),
    ("GetBranch", "GET", "/v1/repos/{owner}/{repo}/branches/{branch}"),
    ("ListCommits", "GET", "/v1/repos/{owner}/{repo}/commits"),
    ("GetCommit", "GET", "/v1/repos/{owner}/{repo}/commits/{sha}"),
    ("GetContents", "GET", "/v1/repos/{owner}/{repo}/contents/{path=**}"),
    ("ListTags", "GET", "/v1/repos/{owner}/{repo}/tags"),
    ("ListLanguages", "GET", "/v1/repos/{owner}/{repo}/languages"),
    // Issues.
    ("ListIssues", "GET", "/v1/repos/{owner}/{repo}/issues"),
    ("CreateIssue", "POST", "/v1/repos/{owner}/{repo}/issues"),
    ("GetIssue", "GET", "/v1/repos/{owner}/{repo}/issues/{issue}"),
    ("UpdateIssue", "PATCH", "/v1/repos/{owner}/{repo}/issues/{issue}"),
    ("ListIssueComments", "GET", "/v1/repos/{owner}/{repo}/issues/{issue}/comments"),
    ("CreateIssueComment", "POST", "/v1/repos/{owner}/{repo}/issues/{issue}/comments"),
    ("GetIssueComment", "GET", "/v1/repos/{owner}/{repo}/issues/{issue}/comments/{comment}"),
    ("ListIssueLabels", "GET", "/v1/repos/{owner}/{repo}/issues/{issue}/labels"),
    // Pull requests.
    ("ListPulls", "GET", "/v1/repos/{owner}/{repo}/pulls"),
    ("CreatePull", "POST", "/v1/repos/{owner}/{repo}/pulls"),
    ("GetPull", "GET", "/v1/repos/{owner}/{repo}/pulls/{pull}"),
    ("UpdatePull", "PATCH", "/v1/repos/{owner}/{repo}/pulls/{pull}"),
    ("ListPullCommits", "GET", "/v1/repos/{owner}/{repo}/pulls/{pull}/commits"),
    ("ListPullFiles", "GET", "/v1/repos/{owner}/{repo}/pulls/{pull}/files"),
    ("MergePull", "PUT", "/v1/repos/{owner}/{repo}/pulls/{pull}/merge"),
    // Organizations.
    ("GetOrg", "GET", "/v1/orgs/{org}"),
    ("ListOrgMembers", "GET", "/v1/orgs/{org}/members"),
    ("GetOrgMember", "GET", "/v1/orgs/{org}/members/{user}"),
    ("ListOrgRepos", "GET", "/v1/orgs/{org}/repos"),
    ("ListOrgTeams", "GET", "/v1/orgs/{org}/teams"),
    ("GetOrgTeam", "GET", "/v1/orgs/{org}/teams/{team}"),
    // Gists.
    ("ListGists", "GET", "/v1/gists"),
    ("CreateGist", "POST", "/v1/gists"),
    ("GetGist", "GET", "/v1/gists/{gist}"),
    ("UpdateGist", "PATCH", "/v1/gists/{gist}"),
    ("ListGistComments", "GET", "/v1/gists/{gist}/comments"),
    // Search.
    ("SearchRepositories", "GET", "/v1/search/repositories"),
    ("SearchIssues", "GET", "/v1/search/issues"),
    ("SearchUsers", "GET", "/v1/search/users"),
    // Static assets (catch-all) and feeds.
    ("GetStatic", "GET", "/v1/static/{path=**}"),
    ("ListFeeds", "GET", "/v1/feeds"),
];
