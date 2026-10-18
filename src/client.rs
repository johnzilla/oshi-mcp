//! Thin HTTP client for the Oshi API Rewards REST API.

use std::{fmt, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::{
    Method, Response, StatusCode,
    header::{AUTHORIZATION, HeaderMap, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use serde_json::Value;
use url::Url;

use crate::config::ApiKey;

pub const DEFAULT_BASE_URL: &str = "https://app.oshi.tech/api/rewards";

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_ERROR_BODY_CHARS: usize = 2000;

#[derive(Clone)]
pub struct OshiClient {
    http: reqwest::Client,
    base: Url,
}

impl OshiClient {
    pub fn new(api_key: &ApiKey, base_url: &str) -> Result<Self> {
        let base = Url::parse(base_url).context("invalid base URL")?;
        if base.cannot_be_a_base() || !matches!(base.scheme(), "https" | "http") {
            bail!("base URL must be an http(s) URL");
        }

        let mut auth = HeaderValue::from_str(&format!("Bearer {}", api_key.expose()))
            .context("OSHI_API_KEY contains characters not allowed in an HTTP header")?;
        auth.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth);

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("oshi-mcp/", env!("CARGO_PKG_VERSION")))
            // The API never redirects; refusing to follow keeps the key on one host.
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http, base })
    }

    /// Builds `{base}/{segments...}`, percent-encoding each segment so caller
    /// supplied ids (including `/`, `?` and `#`) can never change the route.
    /// `url` drops `.` and `..` segments, so callers must reject those.
    pub fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("base URL was checked in OshiClient::new")
            .pop_if_empty()
            .extend(segments);
        url
    }

    pub async fn get(&self, url: Url) -> Result<Value, ApiError> {
        self.send(Method::GET, url, None).await
    }

    pub async fn post(&self, url: Url, body: Option<&Value>) -> Result<Value, ApiError> {
        self.send(Method::POST, url, body).await
    }

    pub async fn patch(&self, url: Url, body: &Value) -> Result<Value, ApiError> {
        self.send(Method::PATCH, url, Some(body)).await
    }

    /// Sends a request and returns the JSON body.
    async fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
    ) -> Result<Value, ApiError> {
        let mut req = self.http.request(method, url);
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ApiError::new(format!("request to Oshi failed: {}", e.without_url())))?;

        let status = resp.status();
        let retry_after = resp
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let bytes = read_capped(resp).await?;

        if !status.is_success() {
            return Err(describe_error(status, retry_after.as_deref(), &bytes));
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::new(format!("Oshi returned invalid JSON ({status}): {e}")))
    }
}

/// A failed API call. The response body is kept apart from the message so
/// callers can filter it (e.g. redact credentials) before it reaches the model.
#[derive(Debug)]
pub struct ApiError {
    pub message: String,
    /// The error response body: parsed JSON, or the raw text as a JSON string.
    pub body: Option<Value>,
}

impl ApiError {
    fn new(message: String) -> Self {
        Self {
            message,
            body: None,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        let body = match &self.body {
            None => return Ok(()),
            Some(Value::String(text)) => text.clone(),
            Some(v) => v.to_string(),
        };
        let shown: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
        write!(f, ": {shown}")?;
        if shown.len() < body.len() {
            f.write_str("…")?;
        }
        Ok(())
    }
}

async fn read_capped(mut resp: Response) -> Result<Vec<u8>, ApiError> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| ApiError::new(format!("reading Oshi response failed: {}", e.without_url())))?
    {
        if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ApiError::new(format!(
                "Oshi response exceeded {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn describe_error(status: StatusCode, retry_after: Option<&str>, body: &[u8]) -> ApiError {
    let mut msg = format!("Oshi API returned {status}");
    match status {
        StatusCode::UNAUTHORIZED => msg.push_str(" (check that OSHI_API_KEY is correct)"),
        StatusCode::TOO_MANY_REQUESTS => match retry_after {
            Some(secs) => msg.push_str(&format!(" (rate limited; retry after {secs} seconds)")),
            None => msg.push_str(" (rate limited; retry after a short delay)"),
        },
        s if s.is_redirection() => msg.push_str(" (unexpected redirect; not followed)"),
        _ => {}
    }
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    let body = (!text.is_empty())
        .then(|| serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned())));
    ApiError { message: msg, body }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path},
    };

    use super::*;

    fn client(base: &str) -> OshiClient {
        OshiClient::new(&ApiKey::new("test_key").unwrap(), base).unwrap()
    }

    #[test]
    fn url_segments_are_encoded() {
        let c = client("https://app.oshi.tech/api/rewards");
        assert_eq!(
            c.url(&["users", "a/b?c#d", "portal-link"]).as_str(),
            "https://app.oshi.tech/api/rewards/users/a%2Fb%3Fc%23d/portal-link"
        );
        // A trailing slash on the base doesn't produce `//`.
        let c = client("https://app.oshi.tech/api/rewards/");
        assert_eq!(
            c.url(&["campaigns"]).as_str(),
            "https://app.oshi.tech/api/rewards/campaigns"
        );
    }

    #[test]
    fn rejects_non_http_base() {
        assert!(OshiClient::new(&ApiKey::new("k").unwrap(), "file:///etc/passwd").is_err());
    }

    #[tokio::test]
    async fn sends_bearer_key_and_json_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/events"))
            .and(header("authorization", "Bearer test_key"))
            .and(body_json(json!({"a": 1})))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let v = c
            .post(c.url(&["events"]), Some(&json!({"a": 1})))
            .await
            .unwrap();
        assert_eq!(v, json!({"ok": true}));
    }

    #[tokio::test]
    async fn does_not_follow_redirects() {
        let server = MockServer::start().await;
        Mock::given(path("/campaigns"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://example.com/steal"),
            )
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let err = c.get(c.url(&["campaigns"])).await.unwrap_err().to_string();
        assert!(err.contains("302") && err.contains("not followed"), "{err}");
    }

    #[tokio::test]
    async fn rate_limit_reports_retry_after() {
        let server = MockServer::start().await;
        Mock::given(path("/campaigns"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "17")
                    .set_body_json(json!({"message": "Rate limit exceeded."})),
            )
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let err = c.get(c.url(&["campaigns"])).await.unwrap_err().to_string();
        assert!(err.contains("retry after 17 seconds"), "{err}");
        assert!(err.contains("Rate limit exceeded."), "{err}");
    }

    #[tokio::test]
    async fn error_body_is_passed_through_and_truncated() {
        let server = MockServer::start().await;
        Mock::given(path("/events"))
            .respond_with(ResponseTemplate::new(422).set_body_string("x".repeat(5000)))
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let err = c
            .post(c.url(&["events"]), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("Oshi API returned 422"));
        assert!(err.len() < 2100, "error body should be truncated");
    }

    #[tokio::test]
    async fn oversized_response_is_rejected() {
        let server = MockServer::start().await;
        Mock::given(path("/campaigns"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(" ".repeat(MAX_RESPONSE_BYTES + 1)),
            )
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let err = c.get(c.url(&["campaigns"])).await.unwrap_err().to_string();
        assert!(err.contains("exceeded"), "{err}");
    }
}
