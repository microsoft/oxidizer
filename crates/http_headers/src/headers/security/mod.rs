// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Browser security policy and transport-security headers.

mod content_security_policy;
mod referrer_policy;
mod strict_transport_security;
mod x_content_type_options;

#[doc(inline)]
pub use content_security_policy::{ContentSecurityPolicy, ContentSecurityPolicyOwned, ContentSecurityPolicyView};
#[doc(inline)]
pub use referrer_policy::{ReferrerPolicy, ReferrerPolicyOwned, ReferrerPolicyTokenView, ReferrerPolicyValue, ReferrerPolicyView};
#[doc(inline)]
pub use strict_transport_security::{
    HstsDirectiveView, StrictTransportSecurity, StrictTransportSecurityBuilder, StrictTransportSecurityOwned, StrictTransportSecurityView,
};
#[doc(inline)]
pub use x_content_type_options::{XContentTypeOptions, XContentTypeOptionsOwned, XContentTypeOptionsView};
