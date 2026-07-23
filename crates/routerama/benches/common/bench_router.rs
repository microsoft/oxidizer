// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// GENERATED FILE — do not edit by hand. Regenerate after editing
// `routes_data.rs` with `scripts/perf_report.rs --regenerate-router`.
//
// Static and dynamic typed routers generated from `routes_data.rs`.

/// Static typed router: `#[resolver]` bakes the trie at compile time and
/// coerces each capture into its field type.
#[::routerama::resolve::resolver]
#[derive(Debug)]
enum BenchRoute<'p> {
    #[route(GET, "/v1/users")]
    ListUsers,
    #[route(GET, "/v1/users/{user}")]
    GetUser { user: String },
    #[route(GET, "/v1/users/{user}/repos")]
    ListUserRepos { user: String },
    #[route(GET, "/v1/users/{user}/followers")]
    ListUserFollowers { user: String },
    #[route(GET, "/v1/users/{user}/following")]
    ListUserFollowing { user: String },
    #[route(GET, "/v1/users/{user}/gists")]
    ListUserGists { user: String },
    #[route(GET, "/v1/users/{user}/starred")]
    ListUserStarred { user: String },
    #[route(GET, "/v1/users/{user}/events")]
    ListUserEvents { user: String },
    #[route(GET, "/v1/users/{user}/received_events")]
    ListUserReceivedEvents { user: String },
    #[route(GET, "/v1/repos/{owner}/{repo}")]
    GetRepo { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/branches")]
    ListBranches { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/branches/{branch}")]
    GetBranch { owner: String, repo: String, branch: &'p str },
    #[route(GET, "/v1/repos/{owner}/{repo}/commits")]
    ListCommits { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/commits/{sha}")]
    GetCommit { owner: String, repo: String, sha: &'p str },
    #[route(GET, "/v1/repos/{owner}/{repo}/tags")]
    ListTags { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/languages")]
    ListLanguages { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/stargazers")]
    ListStargazers { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/subscribers")]
    ListSubscribers { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/issues")]
    ListIssues { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/issues/{issue}")]
    GetIssue { owner: String, repo: String, issue: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/issues/{issue}/comments")]
    ListIssueComments { owner: String, repo: String, issue: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/issues/{issue}/comments/{comment}")]
    GetIssueComment { owner: String, repo: String, issue: u32, comment: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/issues/{issue}/labels")]
    ListIssueLabels { owner: String, repo: String, issue: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/pulls")]
    ListPulls { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/pulls/{pull}")]
    GetPull { owner: String, repo: String, pull: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/pulls/{pull}/commits")]
    ListPullCommits { owner: String, repo: String, pull: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/pulls/{pull}/files")]
    ListPullFiles { owner: String, repo: String, pull: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/pulls/{pull}/merge")]
    MergePull { owner: String, repo: String, pull: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/pulls/{pull}/reviews/{review}")]
    GetPullReview { owner: String, repo: String, pull: u32, review: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/releases")]
    ListReleases { owner: String, repo: String },
    #[route(GET, "/v1/repos/{owner}/{repo}/releases/{release}")]
    GetRelease { owner: String, repo: String, release: u32 },
    #[route(GET, "/v1/repos/{owner}/{repo}/notifications")]
    ListNotifications { owner: String, repo: String },
    #[route(GET, "/v1/orgs/{org}")]
    GetOrg { org: String },
    #[route(GET, "/v1/orgs/{org}/members")]
    ListOrgMembers { org: String },
    #[route(GET, "/v1/orgs/{org}/members/{user}")]
    GetOrgMember { org: String, user: String },
    #[route(GET, "/v1/orgs/{org}/repos")]
    ListOrgRepos { org: String },
    #[route(GET, "/v1/orgs/{org}/teams")]
    ListOrgTeams { org: String },
    #[route(GET, "/v1/orgs/{org}/teams/{team}")]
    GetOrgTeam { org: String, team: &'p str },
    #[route(GET, "/v1/gists")]
    ListGists,
    #[route(GET, "/v1/gists/{gist}")]
    GetGist { gist: &'p str },
    #[route(GET, "/v1/gists/{gist}/comments")]
    ListGistComments { gist: &'p str },
    #[route(GET, "/v1/search/repositories")]
    SearchRepositories,
    #[route(GET, "/v1/search/issues")]
    SearchIssues,
    #[route(GET, "/v1/search/users")]
    SearchUsers,
    #[route(GET, "/v1/feeds")]
    GetFeeds,
    #[route(GET, "/v1/rate_limit")]
    GetRateLimit,
}
/// Dynamic typed router: the same routes registered at run time through the
/// generated builder. Dynamic captures are always owned.
#[::routerama::resolve::resolver]
#[derive(Debug)]
enum BenchDynRoute {
    #[route(dynamic)]
    ListUsers,
    #[route(dynamic)]
    GetUser { user: String },
    #[route(dynamic)]
    ListUserRepos { user: String },
    #[route(dynamic)]
    ListUserFollowers { user: String },
    #[route(dynamic)]
    ListUserFollowing { user: String },
    #[route(dynamic)]
    ListUserGists { user: String },
    #[route(dynamic)]
    ListUserStarred { user: String },
    #[route(dynamic)]
    ListUserEvents { user: String },
    #[route(dynamic)]
    ListUserReceivedEvents { user: String },
    #[route(dynamic)]
    GetRepo { owner: String, repo: String },
    #[route(dynamic)]
    ListBranches { owner: String, repo: String },
    #[route(dynamic)]
    GetBranch { owner: String, repo: String, branch: String },
    #[route(dynamic)]
    ListCommits { owner: String, repo: String },
    #[route(dynamic)]
    GetCommit { owner: String, repo: String, sha: String },
    #[route(dynamic)]
    ListTags { owner: String, repo: String },
    #[route(dynamic)]
    ListLanguages { owner: String, repo: String },
    #[route(dynamic)]
    ListStargazers { owner: String, repo: String },
    #[route(dynamic)]
    ListSubscribers { owner: String, repo: String },
    #[route(dynamic)]
    ListIssues { owner: String, repo: String },
    #[route(dynamic)]
    GetIssue { owner: String, repo: String, issue: u32 },
    #[route(dynamic)]
    ListIssueComments { owner: String, repo: String, issue: u32 },
    #[route(dynamic)]
    GetIssueComment { owner: String, repo: String, issue: u32, comment: u32 },
    #[route(dynamic)]
    ListIssueLabels { owner: String, repo: String, issue: u32 },
    #[route(dynamic)]
    ListPulls { owner: String, repo: String },
    #[route(dynamic)]
    GetPull { owner: String, repo: String, pull: u32 },
    #[route(dynamic)]
    ListPullCommits { owner: String, repo: String, pull: u32 },
    #[route(dynamic)]
    ListPullFiles { owner: String, repo: String, pull: u32 },
    #[route(dynamic)]
    MergePull { owner: String, repo: String, pull: u32 },
    #[route(dynamic)]
    GetPullReview { owner: String, repo: String, pull: u32, review: u32 },
    #[route(dynamic)]
    ListReleases { owner: String, repo: String },
    #[route(dynamic)]
    GetRelease { owner: String, repo: String, release: u32 },
    #[route(dynamic)]
    ListNotifications { owner: String, repo: String },
    #[route(dynamic)]
    GetOrg { org: String },
    #[route(dynamic)]
    ListOrgMembers { org: String },
    #[route(dynamic)]
    GetOrgMember { org: String, user: String },
    #[route(dynamic)]
    ListOrgRepos { org: String },
    #[route(dynamic)]
    ListOrgTeams { org: String },
    #[route(dynamic)]
    GetOrgTeam { org: String, team: String },
    #[route(dynamic)]
    ListGists,
    #[route(dynamic)]
    GetGist { gist: String },
    #[route(dynamic)]
    ListGistComments { gist: String },
    #[route(dynamic)]
    SearchRepositories,
    #[route(dynamic)]
    SearchIssues,
    #[route(dynamic)]
    SearchUsers,
    #[route(dynamic)]
    GetFeeds,
    #[route(dynamic)]
    GetRateLimit,
}
/// Builds the dynamic typed router by registering every benchmark route at
/// run time (part of the non-measured setup step).
#[expect(clippy::too_many_lines, reason = "one fluent call per benchmark route")]
fn build_bench_dyn() -> BenchDynRouteResolver {
    BenchDynRoute::builder()
        .add_list_users(::routerama::resolve::HttpMethod::GET, "/v1/users")
        .add_get_user(::routerama::resolve::HttpMethod::GET, "/v1/users/{user}")
        .add_list_user_repos(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/users/{user}/repos",
        )
        .add_list_user_followers(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/users/{user}/followers",
        )
        .add_list_user_following(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/users/{user}/following",
        )
        .add_list_user_gists(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/users/{user}/gists",
        )
        .add_list_user_starred(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/users/{user}/starred",
        )
        .add_list_user_events(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/users/{user}/events",
        )
        .add_list_user_received_events(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/users/{user}/received_events",
        )
        .add_get_repo(::routerama::resolve::HttpMethod::GET, "/v1/repos/{owner}/{repo}")
        .add_list_branches(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/branches",
        )
        .add_get_branch(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/branches/{branch}",
        )
        .add_list_commits(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/commits",
        )
        .add_get_commit(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/commits/{sha}",
        )
        .add_list_tags(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/tags",
        )
        .add_list_languages(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/languages",
        )
        .add_list_stargazers(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/stargazers",
        )
        .add_list_subscribers(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/subscribers",
        )
        .add_list_issues(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues",
        )
        .add_get_issue(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues/{issue}",
        )
        .add_list_issue_comments(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues/{issue}/comments",
        )
        .add_get_issue_comment(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues/{issue}/comments/{comment}",
        )
        .add_list_issue_labels(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues/{issue}/labels",
        )
        .add_list_pulls(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls",
        )
        .add_get_pull(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}",
        )
        .add_list_pull_commits(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}/commits",
        )
        .add_list_pull_files(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}/files",
        )
        .add_merge_pull(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}/merge",
        )
        .add_get_pull_review(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}/reviews/{review}",
        )
        .add_list_releases(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/releases",
        )
        .add_get_release(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/releases/{release}",
        )
        .add_list_notifications(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/notifications",
        )
        .add_get_org(::routerama::resolve::HttpMethod::GET, "/v1/orgs/{org}")
        .add_list_org_members(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/orgs/{org}/members",
        )
        .add_get_org_member(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/orgs/{org}/members/{user}",
        )
        .add_list_org_repos(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/orgs/{org}/repos",
        )
        .add_list_org_teams(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/orgs/{org}/teams",
        )
        .add_get_org_team(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/orgs/{org}/teams/{team}",
        )
        .add_list_gists(::routerama::resolve::HttpMethod::GET, "/v1/gists")
        .add_get_gist(::routerama::resolve::HttpMethod::GET, "/v1/gists/{gist}")
        .add_list_gist_comments(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/gists/{gist}/comments",
        )
        .add_search_repositories(
            ::routerama::resolve::HttpMethod::GET,
            "/v1/search/repositories",
        )
        .add_search_issues(::routerama::resolve::HttpMethod::GET, "/v1/search/issues")
        .add_search_users(::routerama::resolve::HttpMethod::GET, "/v1/search/users")
        .add_get_feeds(::routerama::resolve::HttpMethod::GET, "/v1/feeds")
        .add_get_rate_limit(::routerama::resolve::HttpMethod::GET, "/v1/rate_limit")
        .build()
        .expect("every dynamic bench route registers")
}
