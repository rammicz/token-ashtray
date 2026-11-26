//! Bounded anonymous HTTPS fetches over the contract's redirect graph.
//!
//! Redirects are never followed automatically. Each observed hop is checked
//! by [`channel`](super::channel) before it is requested, so no Referer is
//! ever produced and no unvalidated host is contacted. Requests carry only
//! the User-Agent and Accept headers. Dropping a returned future cancels its
//! request.
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use reqwest::{Client, Response, StatusCode, header};

use super::channel::{self, DISCOVERY_URL};
use super::{ArtifactRecord, MAX_METADATA, Reject, Sha256, Version};

const TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(not(test))]
const RETRY_DELAY: Duration = Duration::from_secs(5);
#[cfg(test)]
const RETRY_DELAY: Duration = Duration::ZERO;
const USER_AGENT: &str = concat!("TokenAshtray/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// The response broke the contract: redirect graph, size or bytes.
    Rejected(Reject),
    /// An HTTP status that is neither success nor redirect.
    Status(u16),
    /// Connection, TLS, timeout or body read failure.
    Network,
    /// The staging file could not be written.
    Storage,
}

impl FetchError {
    /// Failures worth the contract's one bounded retry.
    fn temporary(self) -> bool {
        match self {
            Self::Network => true,
            Self::Status(status) => matches!(status, 408 | 429 | 500..=599),
            Self::Rejected(_) | Self::Storage => false,
        }
    }
}

impl From<Reject> for FetchError {
    fn from(reject: Reject) -> Self {
        Self::Rejected(reject)
    }
}

/// Raw manifest bundle bytes and the version of the validated discovery hop,
/// which [`verify`](super::verify) binds to as the observed tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub version: Version,
    pub raw: Vec<u8>,
}

enum Reply {
    Body(Response),
    Redirect(String),
}

pub struct Transport {
    client: Client,
    /// Test seam: sends `https://host/path` to `<local>/host/path` instead.
    #[cfg(test)]
    local: Option<String>,
}

impl Transport {
    pub fn new() -> reqwest::Result<Self> {
        Ok(Self {
            client: client(TIMEOUT, true)?,
            #[cfg(test)]
            local: None,
        })
    }

    /// Discovers the latest manifest: the latest URL must redirect to the
    /// exact tagged manifest URL, which may then hop once to the CDN.
    pub async fn discover(&self) -> Result<Discovered, FetchError> {
        retrying(|| async {
            let Reply::Redirect(location) = self.get(DISCOVERY_URL, MAX_METADATA).await? else {
                return Err(Reject("latest-discovery").into());
            };
            let version = channel::tagged_manifest(&location)?;
            let mut raw = Vec::new();
            let response = self.asset(&location, MAX_METADATA).await?;
            stream(response, MAX_METADATA, |chunk| {
                raw.extend_from_slice(chunk);
                Ok(())
            })
            .await?;
            Ok(Discovered { version, raw })
        })
        .await
    }

    /// Streams a verified manifest row's artifact into `path`, replacing it,
    /// and checks the signed size and SHA-256. On any error the file may hold
    /// partial bytes, which the caller must not use. The file is not synced:
    /// a staged file is re-hashed before any use, so a torn write after a
    /// crash only costs a new download.
    pub async fn download(&self, record: &ArtifactRecord, path: &Path) -> Result<(), FetchError> {
        let url = channel::artifact_url(record.target, record.version);
        // A verified record's length is at most MAX_ARTIFACT, which fits usize.
        let limit = usize::try_from(record.length).map_err(|_| Reject("artifact-bytes"))?;
        retrying(|| async {
            let response = self.asset(&url, limit).await?;
            let file = File::create(path).map_err(|_| FetchError::Storage)?;
            let mut file = BufWriter::with_capacity(256 * 1024, file);
            let mut digest = Sha256::new();
            let length = stream(response, limit, |chunk| {
                digest.update(chunk);
                file.write_all(chunk).map_err(|_| FetchError::Storage)
            })
            .await?;
            file.flush().map_err(|_| FetchError::Storage)?;
            Ok(record.check_digest(length as u64, digest.finish())?)
        })
        .await
    }

    /// Fetches an asset URL directly or through exactly one CDN hop.
    async fn asset(&self, url: &str, limit: usize) -> Result<Response, FetchError> {
        let location = match self.get(url, limit).await? {
            Reply::Body(response) => return Ok(response),
            Reply::Redirect(location) => location,
        };
        channel::cdn_hop(&location)?;
        match self.get(&location, limit).await? {
            Reply::Body(response) => Ok(response),
            Reply::Redirect(_) => Err(Reject("asset-redirect-count").into()),
        }
    }

    /// One GET without following redirects. A body declaring more than
    /// `limit` bytes is refused before it is read.
    async fn get(&self, url: &str, limit: usize) -> Result<Reply, FetchError> {
        let response = self
            .client
            .get(self.route(url))
            .header(header::ACCEPT, "application/octet-stream, application/json")
            .send()
            .await
            .map_err(|_| FetchError::Network)?;
        let status = response.status();
        if status.is_redirection() && status != StatusCode::NOT_MODIFIED {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(Reject("redirect-location"))?;
            return Ok(Reply::Redirect(location.to_owned()));
        }
        if status != StatusCode::OK {
            return Err(FetchError::Status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|declared| declared > limit as u64)
        {
            return Err(Reject("response-size").into());
        }
        Ok(Reply::Body(response))
    }

    fn route(&self, url: &str) -> String {
        #[cfg(test)]
        if let (Some(local), Some(rest)) = (&self.local, url.strip_prefix("https://")) {
            return format!("{local}/{rest}");
        }
        url.to_owned()
    }
}

/// Feeds the body to `sink` chunk by chunk and returns its length, failing as
/// soon as it exceeds `limit` bytes.
async fn stream(
    mut response: Response,
    limit: usize,
    mut sink: impl FnMut(&[u8]) -> Result<(), FetchError>,
) -> Result<usize, FetchError> {
    let mut length = 0;
    while let Some(chunk) = response.chunk().await.map_err(|_| FetchError::Network)? {
        length += chunk.len();
        if length > limit {
            return Err(Reject("response-size").into());
        }
        sink(&chunk)?;
    }
    Ok(length)
}

/// Runs `operation`, then once more after a delay if it failed temporarily.
async fn retrying<T, F, Fut>(operation: F) -> Result<T, FetchError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, FetchError>>,
{
    match operation().await {
        Err(error) if error.temporary() => {
            tokio::time::sleep(RETRY_DELAY).await;
            operation().await
        }
        result => result,
    }
}

/// The one client configuration. Only the test seam passes `https_only =
/// false`, to reach its plain-HTTP local origin without a system proxy.
fn client(timeout: Duration, https_only: bool) -> reqwest::Result<Client> {
    let builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .https_only(https_only);
    if https_only {
        builder
    } else {
        builder.no_proxy()
    }
    .build()
}

#[cfg(test)]
pub(super) mod tests;
