//! Thin HTTP client for the Oshi API Rewards REST API.

use std::{error::Error as _, fmt, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::{
    Method, Response, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use serde_json::Value;
use url::Url;

use crate::config::ApiKey;

pub const DEFAULT_BASE_URL: &str = "https://app.oshi.tech/api/rewards";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_ERROR_BODY_CHARS: usize = 2000;

#[derive(Clone)]
pub struct OshiClient {
    http: reqwest::Client,
    base: Url,
    timeout: Duration,
}

impl OshiClient {
    pub fn new(api_key: &ApiKey, base_url: &str) -> Result<Self> {
        Self::with_timeouts(api_key, base_url, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
    }

    /// Like `new`, with explicit connect and whole-request timeouts.
    pub fn with_timeouts(
        api_key: &ApiKey,
        base_url: &str,
        connect: Duration,
        request: Duration,
    ) -> Result<Self> {
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
            .connect_timeout(connect)
            .timeout(request)
            .build()?;
        Ok(Self {
            http,
            base,
            timeout: request,
        })
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
        let resp = req.send().await.map_err(|e| self.transport_error(e))?;

        let status = resp.status();
        let header = |name| {
            resp.headers()
                .get(name)
                .and_then(|v: &HeaderValue| v.to_str().ok())
                .map(str::to_owned)
        };
        let retry_after = header(RETRY_AFTER);
        let content_type = header(CONTENT_TYPE).unwrap_or_default();
        let bytes = self.read_capped(resp).await?;

        if !status.is_success() {
            return Err(describe_error(
                status,
                retry_after.as_deref(),
                &content_type,
                &bytes,
            ));
        }
        // Oshi always answers success with JSON, so anything else means we
        // can't tell what happened, even though the request was accepted.
        let unreadable = |what: String| ApiError {
            outcome_unknown: true,
            ..ApiError::new(format!("Oshi returned {status} but {what}"))
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Err(unreadable("an empty body".into()));
        }
        if is_html(&content_type, &bytes) {
            return Err(unreadable("an HTML page instead of JSON".into()));
        }
        serde_json::from_slice(&bytes).map_err(|e| unreadable(format!("invalid JSON ({e})")))
    }

    async fn read_capped(&self, mut resp: Response) -> Result<Vec<u8>, ApiError> {
        let mut buf = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| self.transport_error(e))? {
            if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(ApiError {
                    outcome_unknown: true,
                    ..ApiError::new(format!("Oshi response exceeded {MAX_RESPONSE_BYTES} bytes"))
                });
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }

    fn transport_error(&self, e: reqwest::Error) -> ApiError {
        // A failed connection means nothing was sent. Anything later (a timeout,
        // a dropped connection) may have happened after Oshi acted on it.
        let (summary, outcome_unknown) = if e.is_connect() {
            ("couldn't connect to Oshi".to_owned(), false)
        } else if e.is_timeout() {
            (
                format!("Oshi didn't respond within {}s", self.timeout.as_secs_f32()),
                true,
            )
        } else {
            ("the connection to Oshi failed".to_owned(), true)
        };
        // reqwest's top-level message is vague; the cause chain says what broke.
        let e = e.without_url();
        let mut detail = e.to_string();
        let mut source = e.source();
        while let Some(cause) = source {
            detail.push_str(": ");
            detail.push_str(&cause.to_string());
            source = cause.source();
        }
        ApiError {
            outcome_unknown,
            ..ApiError::new(format!("{summary} ({detail})"))
        }
    }
}

/// A failed API call. The response body is kept apart from the message so
/// callers can filter it (e.g. redact credentials) before it reaches the model.
#[derive(Debug)]
pub struct ApiError {
    pub message: String,
    /// The error response body: parsed JSON, or the raw text as a JSON string.
    pub body: Option<Value>,
    /// The request may have taken effect even though it reported failure,
    /// e.g. it timed out after being sent, or Oshi's reply was unreadable.
    pub outcome_unknown: bool,
}

impl ApiError {
    fn new(message: String) -> Self {
        Self {
            message,
            body: None,
            outcome_unknown: false,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(body) = &self.body {
            let body = match body {
                Value::String(text) => text.clone(),
                v => v.to_string(),
            };
            let shown: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
            write!(f, ": {shown}")?;
            if shown.len() < body.len() {
                f.write_str("…")?;
            }
        }
        if self.outcome_unknown {
            f.write_str(". The request may or may not have taken effect")?;
        }
        Ok(())
    }
}

fn is_html(content_type: &str, body: &[u8]) -> bool {
    if content_type.to_ascii_lowercase().contains("text/html") {
        return true;
    }
    let start: String = String::from_utf8_lossy(&body[..body.len().min(512)])
        .trim_start()
        .to_ascii_lowercase();
    start.starts_with("<!doctype html") || start.starts_with("<html")
}

fn describe_error(
    status: StatusCode,
    retry_after: Option<&str>,
    content_type: &str,
    body: &[u8],
) -> ApiError {
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
    // Unknown routes and gateway failures come back as web pages, which are
    // just noise to the model.
    if is_html(content_type, body) {
        msg.push_str(
            " with an HTML page instead of JSON (an unknown endpoint, or a gateway error)",
        );
        return ApiError {
            outcome_unknown: status.is_server_error(),
            ..ApiError::new(msg)
        };
    }
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    let body = (!text.is_empty())
        .then(|| serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned())));
    ApiError {
        message: msg,
        body,
        // A 5xx can come from a gateway after the backend already acted.
        outcome_unknown: status.is_server_error(),
    }
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

    #[tokio::test]
    async fn refused_connection_means_nothing_was_sent() {
        // Bind then release a port so nothing is listening on it. (Dropping a
        // MockServer doesn't work: wiremock keeps pooled servers running.)
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let c = client(&format!("http://127.0.0.1:{port}"));
        let err = c.get(c.url(&["campaigns"])).await.unwrap_err();
        assert!(err.message.starts_with("couldn't connect to Oshi"), "{err}");
        assert!(!err.outcome_unknown);
    }

    #[tokio::test]
    async fn timeout_leaves_the_outcome_unknown() {
        let server = MockServer::start().await;
        Mock::given(path("/events"))
            .respond_with(
                ResponseTemplate::new(202)
                    .set_body_json(json!({}))
                    .set_delay(Duration::from_secs(2)),
            )
            .mount(&server)
            .await;
        let key = ApiKey::new("k").unwrap();
        let c = OshiClient::with_timeouts(
            &key,
            &server.uri(),
            Duration::from_secs(2),
            Duration::from_millis(200),
        )
        .unwrap();
        let err = c.post(c.url(&["events"]), None).await.unwrap_err();
        assert!(err.outcome_unknown);
        let text = err.to_string();
        assert!(text.contains("didn't respond within 0.2s"), "{text}");
        assert!(text.ends_with("may or may not have taken effect"), "{text}");
    }

    async fn respond(template: ResponseTemplate) -> ApiError {
        let server = MockServer::start().await;
        Mock::given(path("/campaigns"))
            .respond_with(template)
            .mount(&server)
            .await;
        let c = client(&server.uri());
        c.get(c.url(&["campaigns"])).await.unwrap_err()
    }

    #[tokio::test]
    async fn unreadable_success_bodies_are_errors_with_unknown_outcome() {
        let cases = [
            ("an empty body", ResponseTemplate::new(200)),
            (
                "an empty body",
                ResponseTemplate::new(202).set_body_string(" \n"),
            ),
            (
                "invalid JSON",
                ResponseTemplate::new(200).set_body_raw("{\"campaigns\": [", "application/json"),
            ),
            (
                "an HTML page instead of JSON",
                ResponseTemplate::new(200)
                    .set_body_raw("<!DOCTYPE html><html>hi</html>", "text/html"),
            ),
        ];
        for (expected, template) in cases {
            let err = respond(template).await;
            assert!(err.message.contains(expected), "{expected}: {err}");
            assert!(err.outcome_unknown, "{expected}");
        }
    }

    #[tokio::test]
    async fn html_error_pages_are_summarized_not_passed_through() {
        let page = format!(
            "<!DOCTYPE html><html lang=\"en\">{}</html>",
            "<div>x</div>".repeat(500)
        );
        let err = respond(
            ResponseTemplate::new(404).set_body_raw(page.clone(), "text/html; charset=utf-8"),
        )
        .await;
        let text = err.to_string();
        assert!(
            text.starts_with("Oshi API returned 404 Not Found with an HTML page"),
            "{text}"
        );
        assert!(!text.contains('<'), "{text}");
        assert!(!err.outcome_unknown);

        // Sniffed even without a content type; a 5xx page leaves the outcome unknown.
        let err = respond(ResponseTemplate::new(502).set_body_string(page)).await;
        assert!(
            err.to_string().contains("HTML page") && err.outcome_unknown,
            "{err}"
        );
    }

    #[tokio::test]
    async fn empty_error_body_gives_a_clean_message() {
        let err = respond(ResponseTemplate::new(405)).await;
        assert_eq!(err.to_string(), "Oshi API returned 405 Method Not Allowed");
    }
}
