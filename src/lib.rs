//! Thin async [Resend](https://resend.com) email client shared across bjorst
//! services.
//!
//! The client is bound to a single sender address and posts messages to the
//! Resend HTTP API. Environment policy (when to enable email, which sender to
//! use) stays in the caller; this crate only owns the transport.
//!
//! ```no_run
//! # async fn run() -> Result<(), bjorst_resend::EmailError> {
//! use bjorst_resend::{Email, EmailClient};
//!
//! let client = EmailClient::new("re_123", "noreply@app.dev");
//!
//! // Plain text, the common case.
//! let sent = client.send("user@example.com", "Welcome", "Thanks for signing up").await?;
//! println!("provider message id: {}", sent.id);
//!
//! // Or an HTML body with a plain-text alternative.
//! client
//!     .send_email(Email {
//!         to: "user@example.com",
//!         subject: "Welcome",
//!         text: Some("Thanks for signing up"),
//!         html: Some("<p>Thanks for signing up</p>"),
//!     })
//!     .await?;
//! # Ok(())
//! # }
//! ```

#[cfg(feature = "webhook")]
pub mod webhook;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
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
    /// The message carried neither a text nor an HTML body.
    ///
    /// Resend rejects such a request, so it is caught here rather than spent as
    /// a round trip.
    #[error("resend: message has neither a text nor an HTML body")]
    EmptyBody,
}

/// A message to send.
///
/// At least one of `text` and `html` must be set. Supplying both sends a
/// multipart message and lets the reader's client choose — which is what an
/// institutional mailing usually wants, since some recipients read mail as
/// plain text by policy.
#[derive(Debug, Clone)]
pub struct Email<'a> {
    /// The single recipient address.
    pub to: &'a str,
    /// The subject line.
    pub subject: &'a str,
    /// The plain-text body, if any.
    pub text: Option<&'a str>,
    /// The HTML body, if any.
    pub html: Option<&'a str>,
}

/// What Resend accepted.
///
/// `id` is the provider's message identifier. It is the only thing that ties a
/// send to the delivery, bounce and complaint webhooks that arrive later, so a
/// caller recording delivery state must persist it.
#[derive(Debug, Clone, Deserialize)]
pub struct SentEmail {
    /// The provider's message id.
    pub id: String,
}

/// A thin Resend API client bound to a single sender address.
#[derive(Clone)]
pub struct EmailClient {
    client: Client,
    api_key: String,
    from: String,
}

/// The Resend `POST /emails` request body.
#[derive(Serialize)]
struct ResendPayload<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<&'a str>,
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
    /// Sugar over [`EmailClient::send_email`] for the common case.
    ///
    /// # Errors
    /// Returns [`EmailError::Request`] when the HTTP request fails, or
    /// [`EmailError::Api`] when Resend responds with a non-success status.
    pub async fn send(&self, to: &str, subject: &str, text: &str) -> Result<SentEmail, EmailError> {
        self.send_email(Email {
            to,
            subject,
            text: Some(text),
            html: None,
        })
        .await
    }

    /// Send a message to a single recipient.
    ///
    /// # Errors
    /// Returns [`EmailError::EmptyBody`] when neither body is set,
    /// [`EmailError::Request`] when the HTTP request fails, or
    /// [`EmailError::Api`] when Resend responds with a non-success status.
    pub async fn send_email(&self, email: Email<'_>) -> Result<SentEmail, EmailError> {
        if email.text.is_none() && email.html.is_none() {
            return Err(EmailError::EmptyBody);
        }

        let resp = self
            .client
            .post(RESEND_ENDPOINT)
            .bearer_auth(&self.api_key)
            .json(&ResendPayload {
                from: &self.from,
                to: [email.to],
                subject: email.subject,
                text: email.text,
                html: email.html,
            })
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(EmailError::Api { status, body });
        }

        Ok(resp.json::<SentEmail>().await?)
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
            text: Some("Body"),
            html: None,
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

    /// An absent body is omitted rather than sent as null: Resend rejects
    /// `"html": null`, so serializing it would turn a text-only message into a
    /// 422.
    #[test]
    fn an_unset_body_is_omitted_rather_than_null() {
        let json = serde_json::to_value(ResendPayload {
            from: "noreply@x.app",
            to: ["user@x.app"],
            subject: "Hi",
            text: None,
            html: Some("<p>Body</p>"),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "from": "noreply@x.app",
                "to": ["user@x.app"],
                "subject": "Hi",
                "html": "<p>Body</p>",
            })
        );
        assert!(json.get("text").is_none());
    }

    #[test]
    fn both_bodies_are_sent_when_both_are_given() {
        let json = serde_json::to_value(ResendPayload {
            from: "noreply@x.app",
            to: ["user@x.app"],
            subject: "Hi",
            text: Some("Body"),
            html: Some("<p>Body</p>"),
        })
        .unwrap();
        assert_eq!(json["text"], "Body");
        assert_eq!(json["html"], "<p>Body</p>");
    }

    /// Caught locally rather than spent as a round trip that Resend would
    /// reject anyway.
    #[tokio::test]
    async fn a_message_with_no_body_is_refused_before_the_request() {
        let client = EmailClient::new("re_test", "noreply@x.app");
        let err = client
            .send_email(Email {
                to: "user@x.app",
                subject: "Hi",
                text: None,
                html: None,
            })
            .await
            .expect_err("a bodyless message must be refused");
        assert!(matches!(err, EmailError::EmptyBody));
    }

    /// The provider message id is the only handle on a send once it has left,
    /// so the response has to yield it.
    #[test]
    fn the_response_yields_the_provider_message_id() {
        let sent: SentEmail =
            serde_json::from_value(serde_json::json!({ "id": "49a3999c-0ce1-4ea6-ab68" }))
                .expect("Resend's documented response shape");
        assert_eq!(sent.id, "49a3999c-0ce1-4ea6-ab68");
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
