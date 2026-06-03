//! Browser authentication flow for obtaining a session key.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;

use crate::lastfm::{api_sig, ScrobbleService};

/// Application credentials only — no session key yet.
#[derive(Debug, Clone)]
pub struct AuthClient {
    http: Client,
    api_key: String,
    api_secret: String,
    service: ScrobbleService,
}

impl AuthClient {
    pub fn new(service: ScrobbleService, api_key: String, api_secret: String) -> Self {
        Self {
            http: Client::new(),
            api_key,
            api_secret,
            service,
        }
    }

    pub fn service(&self) -> ScrobbleService {
        self.service
    }

    /// Step 1: request a short-lived token from the API.
    pub async fn get_token(&self) -> Result<String> {
        let mut params = BTreeMap::new();
        params.insert("method".into(), "auth.getToken".into());
        params.insert("api_key".into(), self.api_key.clone());
        params.insert("format".into(), "json".into());

        let body = self.post_signed(params).await?;
        body.get("token")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .context("auth.getToken response missing token")
    }

    /// Step 2: URL the user must open in a browser to approve the app.
    pub fn authorize_url(&self, token: &str) -> String {
        let base = match self.service {
            ScrobbleService::LastFm => "https://www.last.fm/api/auth/",
            ScrobbleService::LibreFm => "https://libre.fm/api/auth/",
        };
        format!("{base}?api_key={}&token={token}", self.api_key)
    }

    /// Step 3: exchange the approved token for a permanent session key.
    pub async fn get_session(&self, token: &str) -> Result<AuthSession> {
        let mut params = BTreeMap::new();
        params.insert("method".into(), "auth.getSession".into());
        params.insert("api_key".into(), self.api_key.clone());
        params.insert("token".into(), token.to_string());
        params.insert("format".into(), "json".into());

        let body = self.post_signed(params).await?;
        let session = body
            .get("session")
            .context("auth.getSession response missing session")?;
        let key = session
            .get("key")
            .and_then(|k| k.as_str())
            .context("session missing key")?
            .to_string();
        let name = session
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        Ok(AuthSession {
            key,
            username: name,
        })
    }

    async fn post_signed(&self, mut params: BTreeMap<String, String>) -> Result<Value> {
        let sig = api_sig(&params, &self.api_secret);
        params.insert("api_sig".into(), sig);

        let resp = self
            .http
            .post(self.service.api_base())
            .form(&params)
            .send()
            .await
            .with_context(|| format!("POST {}", self.service.api_base()))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .with_context(|| format!("parsing {} response", self.service.display_name()))?;

        if let Some(err) = body.get("error") {
            let code = err.as_i64().unwrap_or(-1);
            let message = body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            bail!(
                "{} API error {code}: {message}",
                self.service.display_name()
            );
        }

        if !status.is_success() {
            bail!("{} API HTTP {status}", self.service.display_name());
        }

        Ok(body)
    }
}

/// Result of a successful `auth.getSession` call.
#[derive(Debug, Clone)]
pub struct AuthSession {
    pub key: String,
    pub username: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_lastfm() {
        let client = AuthClient::new(ScrobbleService::LastFm, "abc123".into(), "secret".into());
        assert_eq!(
            client.authorize_url("tok"),
            "https://www.last.fm/api/auth/?api_key=abc123&token=tok"
        );
    }

    #[test]
    fn authorize_url_librefm() {
        let client = AuthClient::new(ScrobbleService::LibreFm, "abc123".into(), "secret".into());
        assert_eq!(
            client.authorize_url("tok"),
            "https://libre.fm/api/auth/?api_key=abc123&token=tok"
        );
    }
}
