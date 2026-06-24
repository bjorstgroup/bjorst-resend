//! Thin async [Resend](https://resend.com) email client shared across bjorst
//! services.
//!
//! The client is bound to a single sender address and posts plain-text emails
//! to the Resend HTTP API. Environment policy (when to enable email, which
//! sender to use) stays in the caller; this crate only owns the transport.
//!
//! ```no_run
//! # async fn run() -> Result<(), bjorst_resend::EmailError> {
//! use bjorst_resend::EmailClient;
//!
//! let client = EmailClient::new("re_123", "noreply@app.dev");
//! client.send("user@example.com", "Welcome", "Thanks for signing up").await?;
//! # Ok(())
//! # }
//! ```

use reqwest::{Client, StatusCode};
use serde::Serialize;
use thiserror::Error;

/// Resend send-email endpoint.
const RESEND_ENDPOINT: &str = "https://api.resend.com/emails";

/// Failure sending an email through Resend.
#[derive(Debug, Error)]
pub enum EmailError {
    /// The HTTP request itself failed (DNS, connection, TLS, timeout).
    #[error("resend: request failed")]
    Request(#[from] reqwest::Error),
    /// Resend returned a non-success status.
    #[error("resend: API returned {status}: {body}")]
    Api {
        /// The HTTP status code returned by Resend.
        status: StatusCode,
        /// The raw response body, for diagnostics.
        body: String,
    },
}

/// A thin Resend API client bound to a single sender address.
#[derive(Clone)]
pub struct EmailClient {
    client: Client,
    api_key: String,
    from: String,
}

/// The Resend `POST /emails` request body for a plain-text message.
#[derive(Serialize)]
struct ResendPayload<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    text: &'a str,
}

impl EmailClient {
    /// Create a client. `from` is the required sender address.
    pub fn new(api_key: impl Into<String>, from: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            from: from.into(),
        }
    }

    /// Send a plain-text email to a single recipient.
    ///
    /// # Errors
    /// Returns [`EmailError::Request`] when the HTTP request fails, or
    /// [`EmailError::Api`] when Resend responds with a non-success status.
    pub async fn send(&self, to: &str, subject: &str, text: &str) -> Result<(), EmailError> {
        let resp = self
            .client
            .post(RESEND_ENDPOINT)
            .bearer_auth(&self.api_key)
            .json(&ResendPayload {
                from: &self.from,
                to: [to],
                subject,
                text,
            })
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(EmailError::Api { status, body });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_serializes_to_resend_shape() {
        let json = serde_json::to_value(ResendPayload {
            from: "noreply@x.app",
            to: ["user@x.app"],
            subject: "Hi",
            text: "Body",
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "from": "noreply@x.app",
                "to": ["user@x.app"],
                "subject": "Hi",
                "text": "Body",
            })
        );
    }

    #[test]
    fn api_error_displays_status_and_body() {
        let err = EmailError::Api {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: "invalid sender".into(),
        };
        assert_eq!(
            err.to_string(),
            "resend: API returned 422 Unprocessable Entity: invalid sender"
        );
    }
}
