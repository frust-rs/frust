//! The default [`ImageFetcher`]: reqwest over rustls (ring provider) with the
//! bundled Mozilla trust store, so no platform verifier is involved on any target.

use std::sync::Arc;
use std::time::Duration;

use crate::fetch::{
    FetchError, FetchFuture, FetchRequest, FetchedBytes, ImageFetcher, validate_url,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// reqwest-backed fetcher. Cheap to clone-share via `Arc`; the inner client pools connections.
pub struct ReqwestFetcher {
    client: reqwest::Client,
}

impl ReqwestFetcher {
    /// Build the default client (30 s timeout, up to 10 redirects, webpki roots).
    pub fn new() -> Result<Self, FetchError> {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }

    /// As [`new`](Self::new) with a custom overall request timeout.
    pub fn with_timeout(timeout: Duration) -> Result<Self, FetchError> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| FetchError::Network(format!("tls config: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .timeout(timeout)
            .user_agent(concat!("frust-network-image/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| FetchError::Network(format!("client build: {}", error_kind(&e))))?;
        Ok(Self { client })
    }

    /// Wrap an app-owned client (its TLS and timeout settings apply).
    pub fn from_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

fn error_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_redirect() {
        "redirect"
    } else if e.is_body() {
        "body"
    } else if e.is_decode() {
        "decode"
    } else if e.is_builder() {
        "builder"
    } else if e.is_request() {
        "request"
    } else {
        "other"
    }
}

fn map_error(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Network(error_kind(&e).to_string())
    }
}

impl ImageFetcher for ReqwestFetcher {
    fn fetch(&self, request: FetchRequest) -> FetchFuture {
        let client = self.client.clone();
        Box::pin(async move {
            validate_url(&request.url)?;
            let mut builder = client.get(&request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            let mut response = builder.send().await.map_err(map_error)?;
            let status = response.status();
            if !status.is_success() {
                return Err(FetchError::Status(status.as_u16()));
            }
            let limit = request.max_bytes;
            if response.content_length().is_some_and(|n| n > limit as u64) {
                return Err(FetchError::TooLarge { limit });
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(map_error)? {
                if bytes.len() + chunk.len() > limit {
                    return Err(FetchError::TooLarge { limit });
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(FetchedBytes {
                bytes,
                content_type,
            })
        })
    }
}
