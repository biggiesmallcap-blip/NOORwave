pub mod identify;
pub mod scanner;

use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::sync::Arc;

type HmacSha1 = Hmac<Sha1>;

#[derive(Debug, Clone)]
pub struct AcrCloudConfig {
    pub access_key: String,
    pub access_secret: String,
    pub region: String, // e.g., "eu-west-1", "us-east-1"
}

#[derive(Debug, Clone)]
pub struct AcrCloudClient {
    pub config: AcrCloudConfig,
    pub http_client: reqwest::Client,
    pub rate_limit_semaphore: Arc<tokio::sync::Semaphore>,
}

impl AcrCloudClient {
    pub fn new(config: AcrCloudConfig, http_client: reqwest::Client) -> Self {
        Self {
            config,
            http_client,
            rate_limit_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.config.access_key.is_empty() && !self.config.access_secret.is_empty()
    }
}
