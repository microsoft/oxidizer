// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for templated URI functionality.

use std::fmt::Display;

#[cfg(feature = "uuid")]
use data_privacy::Sensitive;
use data_privacy::simple_redactor::SimpleRedactor;
use data_privacy::{RedactedToString, RedactionEngine, classified, taxonomy};
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
