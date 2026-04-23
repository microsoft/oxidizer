// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for templated URI functionality.

use std::fmt::Display;

#[cfg(feature = "uuid")]
use data_privacy::Sensitive;
use data_privacy::simple_redactor::SimpleRedactor;
use data_privacy::{RedactedToString, RedactionEngine, classified, taxonomy};
use templated_uri::{BaseUri, PathAndQuery, Uri, UriParam, UriSafeString, UriTemplate, UriUnsafeParam, templated};

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
#[derive(Clone, UriParam)]
struct OrgId(UriSafeString);

#[classified(TestTaxonomy::Eupi)]
#[derive(Clone, UriParam)]
struct UserId(UriSafeString);

#[classified(TestTaxonomy::Public)]
#[derive(Clone, UriUnsafeParam)]
struct PathFragment(String);

#[templated(template = "/{+param}{/param2,param3}{?q1,q2}", unredacted)]
#[derive(Clone)]
struct PathAndQueryTemplate {
    param: String,
    param2: UriSafeString,
    param3: UriSafeString,
    q1: UriSafeString,
    q2: u32,
}

#[test]
fn templated_uri() {
    let test = PathAndQueryTemplate {
        param: "value1".to_string(),
        param2: UriSafeString::from_static("value2"),
        param3: UriSafeString::from_static("value3"),
        q1: UriSafeString::from_static("query1"),
        q2: 42,
    };

    assert_eq!(test.to_uri_string(), "/value1/value2/value3?q1=query1&q2=42");
    assert_eq!(test.rfc_6570_template(), "/{+param}{/param2,param3}{?q1,q2}");
    assert_eq!(format!("{test:?}"), r#"PathAndQueryTemplate("/{+param}{/param2,param3}{?q1,q2}")"#);
}

#[test]
fn uri_with_template() {
    let test = PathAndQueryTemplate {
        param: "value1".to_string(),
        param2: UriSafeString::from_static("value2"),
        param3: UriSafeString::from_static("value3"),
        q1: UriSafeString::from_static("query1"),
        q2: 42,
    };

    let target = Uri::default()
        .with_base(BaseUri::from_static("https://example.com"))
        .with_path(test);
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
        user_id: UserId(UriSafeString::from_static("123e4567-e89b-12d3-a456-426614174000")),
        path_fragment: PathFragment("info/details".to_string()),
    };

    assert_eq!(
        Uri::from(test.clone())
            .with_base(BaseUri::from_static("https://example.com"))
            .to_string()
            .declassify_ref(),
        "https://example.com/users/123e4567-e89b-12d3-a456-426614174000/info/details"
    );

    let target = test.into_uri().with_base(BaseUri::from_static("https://example.com"));

    assert_eq!(
        target.to_string().declassify_ref(),
        "https://example.com/users/123e4567-e89b-12d3-a456-426614174000/info/details"
    );
    assert_eq!(
        format!("{target:?}"),
        r#"Uri { base_uri: BaseUri { origin: Origin { scheme: "https", authority: example.com }, path: BasePath { inner: / } }, path: Some(UriPath(UserInfo("/users/{user_id}/{+path_fragment}"))) }"#
    );
    assert_eq!(target.to_path().unwrap().template(), "/users/{user_id}/{path_fragment}");
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

    let target = user_info.into_uri().with_base(BaseUri::from_static("https://example.com"));

    assert_eq!(
        format!("{target:?}"),
        r#"Uri { base_uri: BaseUri { origin: Origin { scheme: "https", authority: example.com }, path: BasePath { inner: / } }, path: Some(UriPath(ClassifiedUserInfo("/users/{user_id}/info"))) }"#
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

impl UriUnsafeParam for Action {
    fn as_display(&self) -> impl Display {
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
        org_id: OrgId(UriSafeString::from_static("Acme")),
        user_id: UserId(UriSafeString::from_static("Will_E_Coyote")),
        action: Action::Edit,
    });
    assert_eq!(api_edit.to_uri_string(), "/Acme/user/Will_E_Coyote/edit/");
    assert_eq!(
        format!("{api_edit:?}"),
        r#"UserApi(UserActionPath("/{org_id}/user/{user_id}/{+action}/"))"#
    );
    assert_eq!(api_edit.rfc_6570_template(), "/{org_id}/user/{user_id}/{+action}/");
    let api_read = UserApi::UserPath(UserPath {
        org_id: OrgId(UriSafeString::from_static("Acme")),
        user_id: UserId(UriSafeString::from_static("Will_E_Coyote")),
    });
    assert_eq!(api_read.to_uri_string(), "/Acme/user/Will_E_Coyote/");
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
    product_id: UriSafeString,
}

#[test]
fn test_field_level_unredacted() {
    let path = MixedRedactionPath {
        org_id: OrgId(UriSafeString::from_static("Acme")),
        product_id: UriSafeString::from_static("product-123"),
    };

    assert_eq!(path.to_uri_string(), "/Acme/product/product-123/");

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
    query: UriSafeString,
    #[unredacted]
    limit: u32,
}

#[test]
fn test_redacted_query_params() {
    let path = SearchPath {
        org_id: OrgId(UriSafeString::from_static("Acme")),
        query: UriSafeString::from_static("rust"),
        limit: 10,
    };

    assert_eq!(path.to_uri_string(), "/Acme/search?query=rust&limit=10");

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
    org_id: UriSafeString,
    user_id: UriSafeString,
    report_type: UriSafeString,
    year: u32,
    month: u32,
}

#[templated(template = "/simple/{id}", unredacted)]
struct SimplePath {
    id: UriSafeString,
}

#[test]
fn test_label_with_complex_template() {
    let path = ComplexReportPath {
        org_id: UriSafeString::from_static("acme"),
        user_id: UriSafeString::from_static("user123"),
        report_type: UriSafeString::from_static("sales"),
        year: 2024,
        month: 12,
    };

    // Verify the template is still the full RFC 6570 template
    assert_eq!(path.template(), "/{org_id}/user/{user_id}/reports/{report_type}/{year}/{month}");

    // Verify the label is returned
    assert_eq!(path.label(), Some("user_monthly_report"));

    // Verify URI generation still works correctly
    assert_eq!(path.to_uri_string(), "/acme/user/user123/reports/sales/2024/12");
}

#[test]
fn test_label_none_when_not_specified() {
    let path = SimplePath {
        id: UriSafeString::from_static("test"),
    };

    // Without label attribute, label() returns None
    assert_eq!(path.label(), None);

    // Template is still available
    assert_eq!(path.template(), "/simple/{id}");
}

#[test]
fn test_uri_path_label() {
    use templated_uri::UriPath;

    // Test with label
    let complex_path = ComplexReportPath {
        org_id: UriSafeString::from_static("acme"),
        user_id: UriSafeString::from_static("user123"),
        report_type: UriSafeString::from_static("sales"),
        year: 2024,
        month: 12,
    };
    let target_paq: UriPath = complex_path.into();
    assert_eq!(target_paq.label().as_deref(), Some("user_monthly_report"));

    // Test without label
    let simple_path = SimplePath {
        id: UriSafeString::from_static("test"),
    };
    let target_paq: UriPath = simple_path.into();
    assert_eq!(target_paq.label().as_deref(), None);

    // Test with non-templated path
    let static_paq = UriPath::from_static("/static/path");
    assert_eq!(static_paq.label().as_deref(), None);
}

#[test]
fn test_uri_path_from_template() {
    use std::borrow::Cow;

    use templated_uri::UriPath;

    #[templated(template = "/api/{user_id}/posts", label = "user_posts", unredacted)]
    #[derive(Clone)]
    struct UserPosts {
        user_id: UriSafeString,
    }

    let user_posts = UserPosts {
        user_id: UriSafeString::from_static("123"),
    };

    let target_paq = UriPath::from_template(user_posts);

    // Verify template
    assert_eq!(target_paq.template(), "/api/{user_id}/posts");

    // Verify label
    assert_eq!(target_paq.label(), Some(Cow::Borrowed("user_posts")));

    // Verify to_uri_string
    assert_eq!(target_paq.to_uri_string(), "/api/123/posts");

    // Verify to_http_path
    let path = PathAndQuery::try_from(&target_paq).unwrap();
    assert_eq!(path.to_string(), "/api/123/posts");

    // Verify redacted string (unredacted because of unredacted attribute)
    let redaction_engine = RedactionEngine::builder().build();
    assert_eq!(target_paq.to_uri_string_redacted(&redaction_engine), "/api/123/posts");

    // Verify it can be used in a Uri
    let uri = Uri::from(target_paq);
    assert_eq!(uri.to_string().declassify_ref(), "/api/123/posts");
}
