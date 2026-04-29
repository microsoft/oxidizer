use xc_http::client::HttpClient;
use xc_http::request::Request;

#[derive(Clone)]
pub struct RequestService {
    pub built: bool,
}

#[autoresolve::resolvable]
impl RequestService {
    pub fn new(_client: &HttpClient, _request: &Request) -> Self {
        Self { built: true }
    }
}
