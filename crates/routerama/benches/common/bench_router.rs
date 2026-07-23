// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// GENERATED FILE — do not edit by hand. Regenerate after editing
// `routes_data.rs` with `scripts/perf_report.rs --regenerate-router`.
//
// Static and dynamic typed routers generated from `routes_data.rs`.

/// Static typed router: `#[resolver]` bakes the trie at compile time and
/// coerces each capture into its field type.
#[::routerama::resolver]
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
#[::routerama::resolver]
#[derive(Debug)]
enum BenchDynRoute {
    ListUsers,
    GetUser { user: String },
    ListUserRepos { user: String },
    ListUserFollowers { user: String },
    ListUserFollowing { user: String },
    ListUserGists { user: String },
    ListUserStarred { user: String },
    ListUserEvents { user: String },
    ListUserReceivedEvents { user: String },
    GetRepo { owner: String, repo: String },
    ListBranches { owner: String, repo: String },
    GetBranch { owner: String, repo: String, branch: String },
    ListCommits { owner: String, repo: String },
    GetCommit { owner: String, repo: String, sha: String },
    ListTags { owner: String, repo: String },
    ListLanguages { owner: String, repo: String },
    ListStargazers { owner: String, repo: String },
    ListSubscribers { owner: String, repo: String },
    ListIssues { owner: String, repo: String },
    GetIssue { owner: String, repo: String, issue: u32 },
    ListIssueComments { owner: String, repo: String, issue: u32 },
    GetIssueComment { owner: String, repo: String, issue: u32, comment: u32 },
    ListIssueLabels { owner: String, repo: String, issue: u32 },
    ListPulls { owner: String, repo: String },
    GetPull { owner: String, repo: String, pull: u32 },
    ListPullCommits { owner: String, repo: String, pull: u32 },
    ListPullFiles { owner: String, repo: String, pull: u32 },
    MergePull { owner: String, repo: String, pull: u32 },
    GetPullReview { owner: String, repo: String, pull: u32, review: u32 },
    ListReleases { owner: String, repo: String },
    GetRelease { owner: String, repo: String, release: u32 },
    ListNotifications { owner: String, repo: String },
    GetOrg { org: String },
    ListOrgMembers { org: String },
    GetOrgMember { org: String, user: String },
    ListOrgRepos { org: String },
    ListOrgTeams { org: String },
    GetOrgTeam { org: String, team: String },
    ListGists,
    GetGist { gist: String },
    ListGistComments { gist: String },
    SearchRepositories,
    SearchIssues,
    SearchUsers,
    GetFeeds,
    GetRateLimit,
}
/// Builds the dynamic typed router by registering every benchmark route at
/// run time (part of the non-measured setup step).
#[expect(clippy::too_many_lines, reason = "one fluent call per benchmark route")]
fn build_bench_dyn() -> BenchDynRouteResolver {
    BenchDynRoute::builder()
        .add_list_users(::routerama::HttpMethod::GET, "/v1/users")
        .add_get_user(::routerama::HttpMethod::GET, "/v1/users/{user}")
        .add_list_user_repos(::routerama::HttpMethod::GET, "/v1/users/{user}/repos")
        .add_list_user_followers(
            ::routerama::HttpMethod::GET,
            "/v1/users/{user}/followers",
        )
        .add_list_user_following(
            ::routerama::HttpMethod::GET,
            "/v1/users/{user}/following",
        )
        .add_list_user_gists(::routerama::HttpMethod::GET, "/v1/users/{user}/gists")
        .add_list_user_starred(::routerama::HttpMethod::GET, "/v1/users/{user}/starred")
        .add_list_user_events(::routerama::HttpMethod::GET, "/v1/users/{user}/events")
        .add_list_user_received_events(
            ::routerama::HttpMethod::GET,
            "/v1/users/{user}/received_events",
        )
        .add_get_repo(::routerama::HttpMethod::GET, "/v1/repos/{owner}/{repo}")
        .add_list_branches(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/branches",
        )
        .add_get_branch(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/branches/{branch}",
        )
        .add_list_commits(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/commits",
        )
        .add_get_commit(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/commits/{sha}",
        )
        .add_list_tags(::routerama::HttpMethod::GET, "/v1/repos/{owner}/{repo}/tags")
        .add_list_languages(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/languages",
        )
        .add_list_stargazers(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/stargazers",
        )
        .add_list_subscribers(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/subscribers",
        )
        .add_list_issues(::routerama::HttpMethod::GET, "/v1/repos/{owner}/{repo}/issues")
        .add_get_issue(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues/{issue}",
        )
        .add_list_issue_comments(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues/{issue}/comments",
        )
        .add_get_issue_comment(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues/{issue}/comments/{comment}",
        )
        .add_list_issue_labels(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/issues/{issue}/labels",
        )
        .add_list_pulls(::routerama::HttpMethod::GET, "/v1/repos/{owner}/{repo}/pulls")
        .add_get_pull(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}",
        )
        .add_list_pull_commits(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}/commits",
        )
        .add_list_pull_files(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}/files",
        )
        .add_merge_pull(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}/merge",
        )
        .add_get_pull_review(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/pulls/{pull}/reviews/{review}",
        )
        .add_list_releases(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/releases",
        )
        .add_get_release(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/releases/{release}",
        )
        .add_list_notifications(
            ::routerama::HttpMethod::GET,
            "/v1/repos/{owner}/{repo}/notifications",
        )
        .add_get_org(::routerama::HttpMethod::GET, "/v1/orgs/{org}")
        .add_list_org_members(::routerama::HttpMethod::GET, "/v1/orgs/{org}/members")
        .add_get_org_member(
            ::routerama::HttpMethod::GET,
            "/v1/orgs/{org}/members/{user}",
        )
        .add_list_org_repos(::routerama::HttpMethod::GET, "/v1/orgs/{org}/repos")
        .add_list_org_teams(::routerama::HttpMethod::GET, "/v1/orgs/{org}/teams")
        .add_get_org_team(::routerama::HttpMethod::GET, "/v1/orgs/{org}/teams/{team}")
        .add_list_gists(::routerama::HttpMethod::GET, "/v1/gists")
        .add_get_gist(::routerama::HttpMethod::GET, "/v1/gists/{gist}")
        .add_list_gist_comments(
            ::routerama::HttpMethod::GET,
            "/v1/gists/{gist}/comments",
        )
        .add_search_repositories(::routerama::HttpMethod::GET, "/v1/search/repositories")
        .add_search_issues(::routerama::HttpMethod::GET, "/v1/search/issues")
        .add_search_users(::routerama::HttpMethod::GET, "/v1/search/users")
        .add_get_feeds(::routerama::HttpMethod::GET, "/v1/feeds")
        .add_get_rate_limit(::routerama::HttpMethod::GET, "/v1/rate_limit")
        .build()
        .expect("every dynamic bench route registers")
}
