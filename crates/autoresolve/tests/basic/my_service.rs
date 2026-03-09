use autoresolve_macros::resolvable;

use super::client::Client;
use super::config::Config;

pub struct MyService {
    client: Client,
    config: Config,
}

#[resolvable]
impl MyService {
    fn new(client: &Client, config: &Config) -> Self {
        Self {
            client: client.clone(),
            config: config.clone(),
        }
    }

    pub(crate) fn number(&self) -> i32 {
        self.client.number() + self.config.number()
    }
}
