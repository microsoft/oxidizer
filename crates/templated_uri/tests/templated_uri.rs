// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for templated URI functionality.

use std::fmt::Display;

use data_privacy::simple_redactor::SimpleRedactor;
use data_privacy::{RedactedToString, RedactionEngine, Sensitive, classified, taxonomy};
use templated_uri::{BaseUri, Escape, EscapedString, PathAndQueryTemplate, Raw, Uri, templated};

// Local taxonomy for testing purposes, mimicking microsoft_enterprise_data_taxonomy
#[taxonomy(test_taxonomy)]
enum TestTaxonomy {
    /// Organizationally Identifiable Information
    Oii,
    /// End User Pseudonymous Identifier
    Eupi,
    /// Public data
    Public,
}

#[classified(TestTaxonomy::Oii)]
#[derive(Clone, Escape)]
struct OrgId(EscapedString);

#[classified(TestTaxonomy::Eupi)]
#[derive(Clone, Escape)]
struct UserId(EscapedString);

#[classified(TestTaxonomy::Public)]
#[derive(Clone, Raw)]
struct PathFragment(String);

#[templated(template = "/{+param}{/param2,param3}{?q1,q2}", unredacted)]
#[derive(Clone)]
struct TemplatedTestPath {
    param: String,
    param2: EscapedString,
    param3: EscapedString,
    q1: EscapedString,
    q2: u32,
}

#[test]
fn templated_uri() {
    let test = TemplatedTestPath {
        param: "value1".to_string(),
        param2: EscapedString::from_static("value2"),
        param3: EscapedString::from_static("value3"),
        q1: EscapedString::from_static("query1"),
        q2: 42,
    };

    assert_eq!(test.render(), "/value1/value2/value3?q1=query1&q2=42");
    assert_eq!(test.template(), "/{+param}{/param2,param3}{?q1,q2}");
    assert_eq!(format!("{test:?}"), r#"TemplatedTestPath("/{+param}{/param2,param3}{?q1,q2}")"#);
}

#[test]
fn uri_with_template() {
    let test = TemplatedTestPath {
        param: "value1".to_string(),
        param2: EscapedString::from_static("value2"),
        param3: EscapedString::from_static("value3"),
        q1: EscapedString::from_static("query1"),
        q2: 42,
    };

    let target = Uri::default()
        .with_base(BaseUri::from_static("https://example.com"))
        .with_path_and_query(test);
    assert_eq!(
        target.to_string().declassify_ref(),
        "https://example.com/value1/value2/value3?q1=query1&q2=42"
    );
}

static_assertions::assert_impl_all!(UserInfo: Into<Uri>);

#[derive(Clone)]
#[templated(template = "/users/{user_id}/{+path_fragment}")]
struct UserInfo {
    user_id: UserId,
    path_fragment: PathFragment,
}

#[test]
fn user_info_uri() {
    let test = UserInfo {
        user_id: UserId(EscapedString::from_static("123e4567-e89b-12d3-a456-426614174000")),
        path_fragment: PathFragment("info/details".to_string()),
    };

    assert_eq!(
        Uri::from(test.clone())
            .with_base(BaseUri::from_static("https://example.com"))
            .to_string()
            .declassify_ref(),
        "https://example.com/users/123e4567-e89b-12d3-a456-426614174000/info/details"
    );

    let target = Uri::from(test).with_base(BaseUri::from_static("https://example.com"));

    assert_eq!(
        target.to_string().declassify_ref(),
        "https://example.com/users/123e4567-e89b-12d3-a456-426614174000/info/details"
    );
    assert_eq!(
        format!("{target:?}"),
        r#"Uri { base_uri: BaseUri { origin: Origin { scheme: "https", authority: example.com }, path: BasePath { inner: / } }, path_and_query: Some(PathAndQuery(UserInfo("/users/{user_id}/{+path_fragment}"))) }"#
    );
    assert_eq!(target.to_path_and_query().unwrap().template(), "/users/{user_id}/{+path_fragment}");
    assert_eq!(
        target.to_redacted_string(&RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build(),),
        "https://example.com/users/************************************/************",
        "redaction should not change the URI for non-classified fields"
    );
}

#[cfg(feature = "uuid")]
static_assertions::assert_impl_all!(ClassifiedUserInfo: Into<Uri>);

#[cfg(feature = "uuid")]
#[templated(template = "/users/{user_id}/info")]
#[derive(Clone)]
struct ClassifiedUserInfo {
    user_id: Sensitive<uuid::Uuid>,
}

#[cfg(feature = "uuid")]
#[test]
fn test_uri_taxonomy() {
    use uuid::Uuid;
    let user_info = ClassifiedUserInfo {
        user_id: Sensitive::new(Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap(), Uri::DATA_CLASS),
    };

    let target = Uri::from(user_info).with_base(BaseUri::from_static("https://example.com"));

    assert_eq!(
        format!("{target:?}"),
        r#"Uri { base_uri: BaseUri { origin: Origin { scheme: "https", authority: example.com }, path: BasePath { inner: / } }, path_and_query: Some(PathAndQuery(ClassifiedUserInfo("/users/{user_id}/info"))) }"#
    );

    assert_eq!(
        target.to_redacted_string(&RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build(),),
        "https://example.com/users/************************************/info"
    );
}

#[templated(template = "/{org_id}/user/{user_id}/", unredacted)]
#[derive(Clone)]
struct UserPath {
    org_id: OrgId,
    user_id: UserId,
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Edit,
    #[expect(dead_code, reason = "Delete action is not used in this test, but included as an example")]
    Delete,
}

impl Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let action = match self {
            Self::Edit => "edit",
            Self::Delete => "delete",
        };
        write!(f, "{action}")
    }
}

impl Raw for Action {
    fn raw(&self) -> impl Display {
        self
    }
}

#[templated(template = "/{org_id}/user/{user_id}/{+action}/", unredacted)]
#[derive(Clone)]
struct UserActionPath {
    org_id: OrgId,
    user_id: UserId,
    action: Action,
}

#[templated]
#[derive(Clone)]
enum UserApi {
    UserPath(UserPath),
    UserEditPath(UserActionPath),
}

#[test]
fn template_enum() {
    let api_edit = UserApi::UserEditPath(UserActionPath {
        org_id: OrgId(EscapedString::from_static("Acme")),
        user_id: UserId(EscapedString::from_static("Will_E_Coyote")),
        action: Action::Edit,
    });
    assert_eq!(api_edit.render(), "/Acme/user/Will_E_Coyote/edit/");
    assert_eq!(
        format!("{api_edit:?}"),
        r#"UserApi(UserActionPath("/{org_id}/user/{user_id}/{+action}/"))"#
    );
    assert_eq!(api_edit.template(), "/{org_id}/user/{user_id}/{+action}/");
    let api_read = UserApi::UserPath(UserPath {
        org_id: OrgId(EscapedString::from_static("Acme")),
        user_id: UserId(EscapedString::from_static("Will_E_Coyote")),
    });
    assert_eq!(api_read.render(), "/Acme/user/Will_E_Coyote/");
    assert_eq!(format!("{api_read:?}"), r#"UserApi(UserPath("/{org_id}/user/{user_id}/"))"#);

    // Test RedactedDisplay implementation for enums
    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();
    assert_eq!(
        api_edit.to_redacted_string(&redaction_engine),
        "/<CLASSIFIED:test_taxonomy/oii>/user/<CLASSIFIED:test_taxonomy/eupi>/edit/",
        "RedactedDisplay should delegate to variant's RedactedDisplay implementation"
    );
    assert_eq!(
        api_read.to_redacted_string(&redaction_engine),
        "/<CLASSIFIED:test_taxonomy/oii>/user/<CLASSIFIED:test_taxonomy/eupi>/",
        "RedactedDisplay should delegate to variant's RedactedDisplay implementation"
    );
}

#[templated(template = "/{org_id}/product/{product_id}/")]
#[derive(Clone)]
struct MixedRedactionPath {
    org_id: OrgId,
    #[unredacted]
    product_id: EscapedString,
}

#[test]
fn test_field_level_unredacted() {
    let path = MixedRedactionPath {
        org_id: OrgId(EscapedString::from_static("Acme")),
        product_id: EscapedString::from_static("product-123"),
    };

    assert_eq!(path.render(), "/Acme/product/product-123/");

    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();

    // The org_id should be redacted (classified), but product_id should not (marked as unredacted)
    assert_eq!(
        path.to_redacted_string(&redaction_engine),
        "/****/product/product-123/",
        "Field-level unredacted attribute should prevent redaction for that field only"
    );
}

#[templated(template = "/{org_id}/search{?query,limit}")]
#[derive(Clone)]
struct SearchPath {
    org_id: OrgId,
    #[unredacted]
    query: EscapedString,
    #[unredacted]
    limit: u32,
}

#[test]
fn test_redacted_query_params() {
    let path = SearchPath {
        org_id: OrgId(EscapedString::from_static("Acme")),
        query: EscapedString::from_static("rust"),
        limit: 10,
    };

    assert_eq!(path.render(), "/Acme/search?query=rust&limit=10");

    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();

    assert_eq!(
        path.to_redacted_string(&redaction_engine),
        "/****/search?query=rust&limit=10",
        "Redacted display should preserve query param structure (key=value with ? and & delimiters)"
    );
}

/// Test for the label functionality - allows providing a simpler label for telemetry
/// when the template is too complex.
#[templated(
    template = "/{org_id}/user/{user_id}/reports/{report_type}/{year}/{month}",
    label = "user_monthly_report",
    unredacted
)]
struct ComplexReportPath {
    org_id: EscapedString,
    user_id: EscapedString,
    report_type: EscapedString,
    year: u32,
    month: u32,
}

#[templated(template = "/simple/{id}", unredacted)]
struct SimplePath {
    id: EscapedString,
}

#[test]
fn test_label_with_complex_template() {
    let path = ComplexReportPath {
        org_id: EscapedString::from_static("acme"),
        user_id: EscapedString::from_static("user123"),
        report_type: EscapedString::from_static("sales"),
        year: 2024,
        month: 12,
    };

    // Verify the template is still the full RFC 6570 template
    assert_eq!(path.template(), "/{org_id}/user/{user_id}/reports/{report_type}/{year}/{month}");

    // Verify the label is returned
    assert_eq!(path.label(), Some("user_monthly_report"));

    // Verify URI generation still works correctly
    assert_eq!(path.render(), "/acme/user/user123/reports/sales/2024/12");
}

#[test]
fn test_label_none_when_not_specified() {
    let path = SimplePath {
        id: EscapedString::from_static("test"),
    };

    // Without label attribute, label() returns None
    assert_eq!(path.label(), None);

    // Template is still available
    assert_eq!(path.template(), "/simple/{id}");
}

#[test]
fn test_uri_path_label() {
    use templated_uri::PathAndQuery;

    // Test with label
    let complex_path = ComplexReportPath {
        org_id: EscapedString::from_static("acme"),
        user_id: EscapedString::from_static("user123"),
        report_type: EscapedString::from_static("sales"),
        year: 2024,
        month: 12,
    };
    let target_paq: PathAndQuery = complex_path.into();
    assert_eq!(target_paq.label().as_deref(), Some("user_monthly_report"));

    // Test without label
    let simple_path = SimplePath {
        id: EscapedString::from_static("test"),
    };
    let target_paq: PathAndQuery = simple_path.into();
    assert_eq!(target_paq.label().as_deref(), None);

    // Test with non-templated path
    let static_paq = PathAndQuery::from_static("/static/path");
    assert_eq!(static_paq.label().as_deref(), None);
}

#[test]
fn test_uri_path_redacted_debug() {
    use templated_uri::PathAndQuery;

    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();

    // Static path: RedactedDebug omits the inner value entirely.
    let static_path = PathAndQuery::from_static("/sensitive/path?query=secret");
    let mut buf = String::new();
    redaction_engine.redacted_debug(&static_path, &mut buf).unwrap();
    assert_eq!(
        buf, "PathAndQuery",
        "static PathAndQuery's RedactedDebug should not leak the inner value"
    );

    // Templated path: RedactedDebug includes the redacted rendering of the template.
    #[templated(template = "/users/{user_id}/info")]
    #[derive(Clone)]
    struct UserInfoPath {
        user_id: OrgId,
    }
    let templated_path: PathAndQuery = PathAndQuery::from_template(UserInfoPath {
        user_id: OrgId(EscapedString::from_static("acme")),
    });

    let mut buf = String::new();
    redaction_engine.redacted_debug(&templated_path, &mut buf).unwrap();
    assert_eq!(buf, "PathAndQuery(\"/users/****/info\")");
}

#[test]
fn test_uri_path_from_template() {
    use std::borrow::Cow;

    use templated_uri::PathAndQuery;

    #[templated(template = "/api/{user_id}/posts", label = "user_posts", unredacted)]
    #[derive(Clone)]
    struct UserPosts {
        user_id: EscapedString,
    }

    let user_posts = UserPosts {
        user_id: EscapedString::from_static("123"),
    };

    let target_paq = PathAndQuery::from_template(user_posts);

    // Verify template
    assert_eq!(target_paq.template(), "/api/{user_id}/posts");

    // Verify label
    assert_eq!(target_paq.label(), Some(Cow::Borrowed("user_posts")));

    // Verify to_string
    assert_eq!(target_paq.to_string().declassify_ref(), "/api/123/posts");

    // Verify to_path_and_query
    let path = http::uri::PathAndQuery::try_from(&target_paq).unwrap();
    assert_eq!(path.to_string(), "/api/123/posts");

    // Verify redacted string (unredacted because of unredacted attribute)
    let redaction_engine = RedactionEngine::builder().build();
    assert_eq!(target_paq.to_redacted_string(&redaction_engine), "/api/123/posts");

    // Verify it can be used in a Uri
    let uri = Uri::from(target_paq);
    assert_eq!(uri.to_string().declassify_ref(), "/api/123/posts");
}

// ======== RFC 6570 undefined values (Option<T>) tests ========

#[templated(template = "/foo{?topic_id}", unredacted)]
#[derive(Clone)]
struct ListTopics {
    topic_id: Option<u32>,
}

#[test]
fn optional_query_param_some() {
    let path = ListTopics { topic_id: Some(42) };
    assert_eq!(path.render(), "/foo?topic_id=42");
}

#[test]
fn optional_query_param_none() {
    let path = ListTopics { topic_id: None };
    assert_eq!(path.render(), "/foo");
}

#[templated(template = "/search{?query,limit,offset}", unredacted)]
#[derive(Clone)]
struct SearchQuery {
    query: EscapedString,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[test]
fn optional_query_mixed_all_some() {
    let path = SearchQuery {
        query: EscapedString::from_static("rust"),
        limit: Some(10),
        offset: Some(20),
    };
    assert_eq!(path.render(), "/search?query=rust&limit=10&offset=20");
}

#[test]
fn optional_query_mixed_some_none() {
    let path = SearchQuery {
        query: EscapedString::from_static("rust"),
        limit: Some(10),
        offset: None,
    };
    assert_eq!(path.render(), "/search?query=rust&limit=10");
}

#[test]
fn optional_query_mixed_all_none() {
    let path = SearchQuery {
        query: EscapedString::from_static("rust"),
        limit: None,
        offset: None,
    };
    assert_eq!(path.render(), "/search?query=rust");
}

// Optional-first ordering: `{?opt,req}` with `opt=None` must still attach the `?` prefix
// to the required value (i.e., not lose the group prefix when the first member is undefined).
// This exercises the `__first` tracking in the order required-after-optional rather than
// the more common optional-after-required.
#[templated(template = "/items{?opt,req}", unredacted)]
#[derive(Clone)]
struct OptionalFirstThenRequired {
    opt: Option<u32>,
    req: u32,
}

#[test]
fn optional_first_then_required_some() {
    let path = OptionalFirstThenRequired { opt: Some(1), req: 2 };
    assert_eq!(path.render(), "/items?opt=1&req=2");
}

#[test]
fn optional_first_then_required_none_attaches_prefix_to_required() {
    let path = OptionalFirstThenRequired { opt: None, req: 2 };
    // `opt` is undefined, so the `?` prefix must hop to `req`. The naive bug would
    // drop the prefix entirely and produce `/itemsreq=2` or `/items&req=2`.
    assert_eq!(path.render(), "/items?req=2");
}

#[templated(template = "/items{?x,y}", unredacted)]
#[derive(Clone)]
struct AllOptionalQuery {
    x: Option<u32>,
    y: Option<u32>,
}

#[test]
fn optional_query_all_optional_both_some() {
    let path = AllOptionalQuery {
        x: Some(1024),
        y: Some(768),
    };
    assert_eq!(path.render(), "/items?x=1024&y=768");
}

#[test]
fn optional_query_all_optional_first_none() {
    // RFC 6570: {?undef,y} → ?y=768
    let path = AllOptionalQuery { x: None, y: Some(768) };
    assert_eq!(path.render(), "/items?y=768");
}

#[test]
fn optional_query_all_optional_second_none() {
    // RFC 6570: {?x,undef} → ?x=1024
    let path = AllOptionalQuery { x: Some(1024), y: None };
    assert_eq!(path.render(), "/items?x=1024");
}

#[test]
fn optional_query_all_optional_both_none() {
    let path = AllOptionalQuery { x: None, y: None };
    assert_eq!(path.render(), "/items");
}

#[templated(template = "/{x}", unredacted)]
#[derive(Clone)]
struct SimpleOptional {
    x: Option<EscapedString>,
}

#[test]
fn optional_simple_expansion_some() {
    let path = SimpleOptional {
        x: Some(EscapedString::from_static("hello")),
    };
    assert_eq!(path.render(), "/hello");
}

#[test]
fn optional_simple_expansion_none() {
    // RFC 6570: O{undef}X → OX — the variable disappears
    let path = SimpleOptional { x: None };
    assert_eq!(path.render(), "/");
}

#[templated(template = "/prefix{/a,b}", unredacted)]
#[derive(Clone)]
struct PathExpansionOptional {
    a: EscapedString,
    b: Option<EscapedString>,
}

#[test]
fn optional_path_expansion_some() {
    let path = PathExpansionOptional {
        a: EscapedString::from_static("x"),
        b: Some(EscapedString::from_static("y")),
    };
    assert_eq!(path.render(), "/prefix/x/y");
}

#[test]
fn optional_path_expansion_none() {
    let path = PathExpansionOptional {
        a: EscapedString::from_static("x"),
        b: None,
    };
    assert_eq!(path.render(), "/prefix/x");
}

#[templated(template = "/data{?required}{&opt}", unredacted)]
#[derive(Clone)]
struct QueryContinuationOptional {
    required: u32,
    opt: Option<u32>,
}

#[test]
fn optional_query_continuation_some() {
    let path = QueryContinuationOptional { required: 1, opt: Some(2) };
    assert_eq!(path.render(), "/data?required=1&opt=2");
}

#[test]
fn optional_query_continuation_none() {
    let path = QueryContinuationOptional { required: 1, opt: None };
    assert_eq!(path.render(), "/data?required=1");
}

#[templated(template = "/{x,y}", unredacted)]
#[derive(Clone)]
struct SimpleMultiOptional {
    x: Option<EscapedString>,
    y: Option<EscapedString>,
}

#[test]
fn optional_simple_multi_both_some() {
    let path = SimpleMultiOptional {
        x: Some(EscapedString::from_static("a")),
        y: Some(EscapedString::from_static("b")),
    };
    assert_eq!(path.render(), "/a,b");
}

#[test]
fn optional_simple_multi_first_none() {
    let path = SimpleMultiOptional {
        x: None,
        y: Some(EscapedString::from_static("b")),
    };
    assert_eq!(path.render(), "/b");
}

#[test]
fn optional_simple_multi_second_none() {
    let path = SimpleMultiOptional {
        x: Some(EscapedString::from_static("a")),
        y: None,
    };
    assert_eq!(path.render(), "/a");
}

#[test]
fn optional_simple_multi_both_none() {
    let path = SimpleMultiOptional { x: None, y: None };
    assert_eq!(path.render(), "/");
}

// Test that Some("") (empty string) is NOT treated as undefined — it's a defined empty value.
// RFC 6570 section 3.2.2: ?{x,empty} → ?1024,  vs  ?{x,undef} → ?1024
#[templated(template = "/?{x,y}", unredacted)]
#[derive(Clone)]
struct SimpleMultiEmptyVsUndef {
    x: EscapedString,
    y: Option<EscapedString>,
}

#[test]
fn optional_some_empty_vs_none_simple() {
    // Defined empty value: separator is kept, value is empty → trailing comma
    let some_empty = SimpleMultiEmptyVsUndef {
        x: EscapedString::from_static("1024"),
        y: Some(EscapedString::from_static("")),
    };
    assert_eq!(some_empty.render(), "/?1024,");

    // Undefined: variable AND separator are omitted
    let none = SimpleMultiEmptyVsUndef {
        x: EscapedString::from_static("1024"),
        y: None,
    };
    assert_eq!(none.render(), "/?1024");
}

#[test]
fn optional_some_empty_vs_none() {
    let some_empty = ListTopics { topic_id: Some(0) };
    assert_eq!(some_empty.render(), "/foo?topic_id=0");

    let none = ListTopics { topic_id: None };
    assert_eq!(none.render(), "/foo");
}

// Test Option with reserved expansion {+var}
#[templated(template = "/{+path}", unredacted)]
#[derive(Clone)]
struct OptionalReserved {
    path: Option<String>,
}

#[test]
fn optional_reserved_expansion_some() {
    let path = OptionalReserved {
        path: Some("a/b/c".to_string()),
    };
    assert_eq!(path.render(), "/a/b/c");
}

#[test]
fn optional_reserved_expansion_none() {
    let path = OptionalReserved { path: None };
    assert_eq!(path.render(), "/");
}

// Test Option with classified/redacted fields
#[templated(template = "/{org_id}/items{?filter}")]
#[derive(Clone)]
struct RedactedOptionalPath {
    org_id: OrgId,
    #[unredacted]
    filter: Option<EscapedString>,
}

#[test]
fn optional_redacted_display_some() {
    let path = RedactedOptionalPath {
        org_id: OrgId(EscapedString::from_static("Acme")),
        filter: Some(EscapedString::from_static("active")),
    };

    assert_eq!(path.render(), "/Acme/items?filter=active");

    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();
    assert_eq!(
        path.to_redacted_string(&redaction_engine),
        "/****/items?filter=active",
        "org_id should be redacted, filter should not"
    );
}

#[test]
fn optional_redacted_display_none() {
    let path = RedactedOptionalPath {
        org_id: OrgId(EscapedString::from_static("Acme")),
        filter: None,
    };

    assert_eq!(path.render(), "/Acme/items");

    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();
    assert_eq!(
        path.to_redacted_string(&redaction_engine),
        "/****/items",
        "undefined filter should be omitted from redacted display too"
    );
}

// Test Option<Sensitive<T>>
#[templated(template = "/users{?user_id}")]
#[derive(Clone)]
struct OptionalSensitiveField {
    user_id: Option<Sensitive<EscapedString>>,
}

#[test]
fn optional_sensitive_some() {
    let path = OptionalSensitiveField {
        user_id: Some(Sensitive::new(EscapedString::from_static("secret_user"), Uri::DATA_CLASS)),
    };
    assert_eq!(path.render(), "/users?user_id=secret_user");

    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();
    assert_eq!(
        path.to_redacted_string(&redaction_engine),
        "/users?user_id=***********",
        "Sensitive value should be redacted"
    );
}

#[test]
fn optional_sensitive_none() {
    let path = OptionalSensitiveField { user_id: None };
    assert_eq!(path.render(), "/users");

    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();
    assert_eq!(
        path.to_redacted_string(&redaction_engine),
        "/users",
        "None value should be omitted from redacted display"
    );
}

// Verifies that `#[unredacted]` actually relaxes the `RedactedDisplay` trait bound to
// just `Display`. `DisplayOnly` deliberately does not implement `RedactedDisplay`, so
// the `redacted_display_group_with_optional` codegen MUST take the `Display`-only branch
// for the marked fields. Flipping `unredacted || field_unredacted` to `&&` in the macro
// would route through `<DisplayOnly as RedactedDisplay>::fmt(...)`, which doesn't exist —
// causing this file to fail compilation. That's the precise semantic invariant the test
// pins down: `#[unredacted]` allows `Display`-only types in templates.
#[derive(Clone)]
struct DisplayOnly(&'static str);

impl Display for DisplayOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl templated_uri::Escape for DisplayOnly {
    fn escape(&self) -> templated_uri::Escaped<impl Display> {
        templated_uri::Escaped::from_static(self.0)
    }
}

#[templated(template = "/items{?required_id,opt_id}")]
#[derive(Clone)]
struct UnredactedDisplayOnlyOptional {
    #[unredacted]
    required_id: DisplayOnly,
    #[unredacted]
    opt_id: Option<DisplayOnly>,
}

#[test]
fn optional_field_unredacted_required_uses_display_only_trait_bound() {
    // `opt_id = None` so the optional-aware path is taken with only `required_id`.
    let path = UnredactedDisplayOnlyOptional {
        required_id: DisplayOnly("req"),
        opt_id: None,
    };
    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();
    assert_eq!(path.to_redacted_string(&redaction_engine), "/items?required_id=req");
}

#[test]
fn optional_field_unredacted_optional_uses_display_only_trait_bound() {
    // `opt_id = Some(...)` covers the optional-field branch.
    let path = UnredactedDisplayOnlyOptional {
        required_id: DisplayOnly("req"),
        opt_id: Some(DisplayOnly("opt")),
    };
    let redaction_engine = RedactionEngine::builder().set_fallback_redactor(SimpleRedactor::new()).build();
    assert_eq!(path.to_redacted_string(&redaction_engine), "/items?required_id=req&opt_id=opt");
}

// Test that template() and format_template() are unchanged by Option usage
#[test]
fn optional_template_metadata_unchanged() {
    let path = AllOptionalQuery { x: None, y: None };
    assert_eq!(path.template(), "/items{?x,y}");
    assert_eq!(path.format_template(), "/items?x={x}&y={y}");
    assert_eq!(format!("{path:?}"), r#"AllOptionalQuery("/items{?x,y}")"#);
}

// ======== Matrix params `{;a,b}` (RFC 6570 section 3.2.7) ========
#[templated(template = "/items{;a,b}", unredacted)]
#[derive(Clone)]
struct MatrixOptional {
    a: Option<EscapedString>,
    b: Option<EscapedString>,
}

#[test]
fn optional_matrix_both_some() {
    let path = MatrixOptional {
        a: Some(EscapedString::from_static("x")),
        b: Some(EscapedString::from_static("y")),
    };
    assert_eq!(path.render(), "/items;a=x;b=y");
}

#[test]
fn optional_matrix_first_none() {
    // RFC 6570: undefined first variable → prefix attaches to second
    let path = MatrixOptional {
        a: None,
        b: Some(EscapedString::from_static("y")),
    };
    assert_eq!(path.render(), "/items;b=y");
}

#[test]
fn optional_matrix_second_none() {
    let path = MatrixOptional {
        a: Some(EscapedString::from_static("x")),
        b: None,
    };
    assert_eq!(path.render(), "/items;a=x");
}

#[test]
fn optional_matrix_both_none() {
    let path = MatrixOptional { a: None, b: None };
    assert_eq!(path.render(), "/items");
}

// ======== Label-style `{.a,b}` (RFC 6570 section 3.2.5) ========
#[templated(template = "/file{.ext1,ext2}", unredacted)]
#[derive(Clone)]
struct LabelOptional {
    ext1: Option<EscapedString>,
    ext2: Option<EscapedString>,
}

#[test]
fn optional_label_both_some() {
    let path = LabelOptional {
        ext1: Some(EscapedString::from_static("tar")),
        ext2: Some(EscapedString::from_static("gz")),
    };
    assert_eq!(path.render(), "/file.tar.gz");
}

#[test]
fn optional_label_first_none() {
    // RFC 6570: undefined first variable → label dot attaches to second
    let path = LabelOptional {
        ext1: None,
        ext2: Some(EscapedString::from_static("gz")),
    };
    assert_eq!(path.render(), "/file.gz");
}

#[test]
fn optional_label_second_none() {
    let path = LabelOptional {
        ext1: Some(EscapedString::from_static("tar")),
        ext2: None,
    };
    assert_eq!(path.render(), "/file.tar");
}

#[test]
fn optional_label_both_none() {
    let path = LabelOptional { ext1: None, ext2: None };
    assert_eq!(path.render(), "/file");
}

#[templated(template = "/users/{user_id}/posts/{post_id}", unredacted)]
#[derive(Clone)]
struct MaterializePath {
    user_id: u32,
    post_id: EscapedString,
}

#[templated(template = "/{+catch_all}", unredacted)]
#[derive(Clone)]
struct CatchAllPath {
    // `{+catch_all}` is a reserved expansion, so a value beginning with `/` renders a
    // second leading slash (`//...`) that base joining must normalize.
    catch_all: String,
}

/// The per-request hot path (base plus templated path into an `http::Uri`) must produce
/// exactly the same URI as materializing the rendered path into a static `PathAndQuery`
/// and joining that. This guards the single-pass `join_rendered` optimization against the
/// original materialize-then-join behavior.
#[test]
fn materialize_hot_path_matches_static_join() {
    let bases = [
        "https://api.example.com",
        "https://api.example.com/",
        "https://api.example.com/v1/",
        "http://localhost:8080/deep/base/",
    ];

    for base_str in bases {
        let base = BaseUri::from_static(base_str);

        // A normal templated path.
        let templated = MaterializePath {
            user_id: 42,
            post_id: EscapedString::from_static("hello-world"),
        };
        let rendered = templated.render();

        let hot: http::Uri = Uri::default()
            .with_base(base.clone())
            .with_path_and_query(templated.clone())
            .try_into()
            .expect("hot-path materialization should succeed");

        let expected: http::Uri = Uri::default()
            .with_base(base.clone())
            .with_path_and_query(http::uri::PathAndQuery::try_from(rendered.as_str()).unwrap())
            .try_into()
            .expect("static-join materialization should succeed");

        assert_eq!(hot, expected, "mismatch for base {base_str:?} (normal path)");

        // A reserved-expansion path whose value begins with `/`, producing `//` before join.
        let catch_all = CatchAllPath {
            catch_all: "/nested/resource?x=1".to_string(),
        };
        let rendered_ca = catch_all.render();
        assert!(rendered_ca.starts_with("//"), "sanity: catch-all should render a double slash");

        let hot_ca: http::Uri = Uri::default()
            .with_base(base.clone())
            .with_path_and_query(catch_all.clone())
            .try_into()
            .expect("hot-path materialization should succeed");

        let expected_ca: http::Uri = Uri::default()
            .with_base(base.clone())
            .with_path_and_query(http::uri::PathAndQuery::try_from(rendered_ca.as_str()).unwrap())
            .try_into()
            .expect("static-join materialization should succeed");

        assert_eq!(hot_ca, expected_ca, "mismatch for base {base_str:?} (catch-all path)");
    }
}

#[templated(template = "/search{?query,limit,offset}", unredacted)]
#[derive(Clone)]
struct CapacityHintPath {
    query: EscapedString,
    limit: u32,
    offset: u32,
}

#[test]
fn macro_render_capacity_hint_exact_value() {
    // The `#[templated]` macro emits a compile-time `render_capacity_hint()` that sums, for
    // `/search{?query,limit,offset}`:
    //   - content "/search"                                        = 7
    //   - group `{?query,limit,offset}` prefix "?"                 = 1
    //   - separators between 3 values (2 x "&", 1 byte each)       = 2
    //   - `key=` literals: "query="(6) + "limit="(6) + "offset="(7) = 19
    //   - 3 values x ESTIMATED_VALUE_LEN (16)                       = 48
    // for a total of 77. Asserting the exact value pins the macro's capacity arithmetic so a
    // mutation to any of the `+`/`*` operators (which does not change rendered output and so
    // is invisible to behavioral tests) is caught here.
    let p = CapacityHintPath {
        query: EscapedString::from_static("x"),
        limit: 1,
        offset: 2,
    };
    assert_eq!(PathAndQueryTemplate::render_capacity_hint(&p), 77);
    // Sanity: the hint must be a genuine upper bound for this concrete (short) render.
    assert!(PathAndQueryTemplate::render_capacity_hint(&p) >= p.render().len());
}

#[test]
fn macro_render_capacity_hint_covers_simple_and_reserved_expansions() {
    // `/{org_id}/user/{user_id}/{+action}/` mixes static content with simple (`{org_id}`,
    // `{user_id}`) and reserved (`{+action}`) expansions, none of which carry a prefix or
    // `key=` literal. Its hint is:
    //   "/"(1) + org_id(16) + "/user/"(6) + user_id(16) + "/"(1) + action(16) + "/"(1) = 57.
    // The enum's generated `render_capacity_hint` must delegate to the active variant, so the
    // value matches the underlying struct's hint. This pins the content and per-value
    // arithmetic for the non-key/value expansion forms.
    let action = UserActionPath {
        org_id: OrgId(EscapedString::from_static("Acme")),
        user_id: UserId(EscapedString::from_static("Will_E_Coyote")),
        action: Action::Edit,
    };
    assert_eq!(PathAndQueryTemplate::render_capacity_hint(&action), 57);

    let enum_variant = UserApi::UserEditPath(action);
    assert_eq!(PathAndQueryTemplate::render_capacity_hint(&enum_variant), 57);
}

#[test]
fn macro_render_into_appends_without_reallocating_prefix() {
    // The macro's `render_into` appends field values directly into a caller-provided buffer
    // rather than allocating a fresh `String`. It must leave any existing prefix intact and
    // append exactly what `render()` would have produced. Guards the macro-generated
    // `render_into` body against a mutation that drops the append statements.
    let p = CapacityHintPath {
        query: EscapedString::from_static("term"),
        limit: 10,
        offset: 5,
    };
    let mut buf = String::from("/prefix");
    PathAndQueryTemplate::render_into(&p, &mut buf);
    assert_eq!(buf, format!("/prefix{}", p.render()));
    assert_eq!(buf, "/prefix/search?query=term&limit=10&offset=5");

    // The enum variant's `render_into` must delegate to the active variant identically.
    let action = UserApi::UserEditPath(UserActionPath {
        org_id: OrgId(EscapedString::from_static("Acme")),
        user_id: UserId(EscapedString::from_static("Wile")),
        action: Action::Edit,
    });
    let mut enum_buf = String::from("base:");
    PathAndQueryTemplate::render_into(&action, &mut enum_buf);
    assert_eq!(enum_buf, format!("base:{}", action.render()));
}

static REF_ID_VALUE: u32 = 4242;

#[templated(template = "/items/{id}{?maybe}", unredacted)]
#[derive(Clone)]
struct ReferenceFieldPath {
    id: &'static u32,
    maybe: Option<&'static u32>,
}

#[test]
fn reference_fields_render_in_required_and_optional_positions() {
    // A `&T` field (where the owned `T: Escape`) must render identically whether it appears
    // in a required (`{id}`) or an optional (`{?maybe}`) position. The macro passes
    // `self.field` (already `&T`) in the required path and `*__val` in the optional path, so
    // both resolve `Escape` on the owned `T` (e.g. `u32`), not on `&T` — the latter has no
    // impl. This pins the required/optional receiver consistency the reviewer flagged.
    static OTHER: u32 = 7;

    let with_opt = ReferenceFieldPath {
        id: &REF_ID_VALUE,
        maybe: Some(&OTHER),
    };
    assert_eq!(with_opt.render(), "/items/4242?maybe=7");

    let without_opt = ReferenceFieldPath {
        id: &REF_ID_VALUE,
        maybe: None,
    };
    assert_eq!(without_opt.render(), "/items/4242");
}
