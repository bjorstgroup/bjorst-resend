//! Svix-signed webhook verification (the `webhook` feature).
//!
//! Resend signs its webhook deliveries with Svix: an HMAC-SHA256 over
//! `"{svix-id}.{svix-timestamp}.{body}"`, keyed with the base64 secret that
//! follows the `whsec_` prefix, sent in `svix-signature` as one or more
//! space-separated `v1,<base64>` entries (more than one while a secret is being
//! rotated).
//!
//! Ported from `bjorst-clerk-axum`'s `webhook` module, which Gakuin borrowed for
//! Resend after it stopped using Clerk: the product that sends these webhooks is
//! this library's, so the check lives here. One change on the way: signatures
//! are compared in constant time (`Mac::verify_slice`), where the original
//! compared base64 strings.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;

/// How far a delivery's timestamp may sit from now, either side, in seconds.
pub const TOLERANCE_SECS: i64 = 300;

/// Why a delivery was refused. Messages never carry the secret.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WebhookError {
    /// `svix-timestamp` is not a whole number of seconds.
    #[error("svix-timestamp is not a number")]
    InvalidTimestamp,
    /// The delivery is older or newer than the replay window allows.
    #[error("svix-timestamp is outside the replay window")]
    TimestampTolerance,
    /// The system clock is before the Unix epoch.
    #[error("system clock error: {0}")]
    Clock(String),
    /// The webhook secret, after `whsec_`, is not base64.
    #[error("failed to decode webhook secret")]
    SecretDecode,
    /// The decoded secret cannot key an HMAC.
    #[error("webhook secret is not a usable HMAC key")]
    HmacKey,
    /// No `v1` signature in the header matches the delivery.
    #[error("signature mismatch")]
    SignatureMismatch,
}

/// Verify one delivery against the current clock.
///
/// * `secret` — the endpoint's signing secret, `whsec_…` (the prefix is
///   optional).
/// * `payload` — the raw request body, byte for byte as received.
/// * `svix_id`, `svix_timestamp`, `svix_signature` — the three headers.
pub fn verify_webhook(
    secret: &str,
    payload: &[u8],
    svix_id: &str,
    svix_timestamp: &str,
    svix_signature: &str,
) -> Result<(), WebhookError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| WebhookError::Clock(e.to_string()))?
        .as_secs();
    verify_webhook_at(
        secret,
        payload,
        svix_id,
        svix_timestamp,
        svix_signature,
        i64::try_from(now).unwrap_or(i64::MAX),
    )
}

/// [`verify_webhook`] against a given `now` (Unix seconds), for tests and for
/// callers that hold their own clock.
pub fn verify_webhook_at(
    secret: &str,
    payload: &[u8],
    svix_id: &str,
    svix_timestamp: &str,
    svix_signature: &str,
    now: i64,
) -> Result<(), WebhookError> {
    let ts: i64 = svix_timestamp
        .parse()
        .map_err(|_| WebhookError::InvalidTimestamp)?;
    if (now - ts).abs() > TOLERANCE_SECS {
        return Err(WebhookError::TimestampTolerance);
    }

    let b64 = base64::engine::general_purpose::STANDARD;
    let key = b64
        .decode(secret.trim_start_matches("whsec_"))
        .map_err(|_| WebhookError::SecretDecode)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| WebhookError::HmacKey)?;
    mac.update(svix_id.as_bytes());
    mac.update(b".");
    mac.update(svix_timestamp.as_bytes());
    mac.update(b".");
    mac.update(payload);

    let matched = svix_signature
        .split(' ')
        .filter_map(|part| part.strip_prefix("v1,"))
        .filter_map(|sig| b64.decode(sig).ok())
        .any(|sig| mac.clone().verify_slice(&sig).is_ok());
    if matched {
        Ok(())
    } else {
        Err(WebhookError::SignatureMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn secret(key: &[u8]) -> String {
        format!(
            "whsec_{}",
            base64::engine::general_purpose::STANDARD.encode(key)
        )
    }

    /// A signature made the way Svix makes one.
    fn sign(key: &[u8], id: &str, ts: &str, body: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
        mac.update(format!("{id}.{ts}.{body}").as_bytes());
        format!(
            "v1,{}",
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
        )
    }

    #[test]
    fn accepts_a_genuine_signature() {
        let body = r#"{"type":"email.delivered"}"#;
        let sig = sign(b"signing-key", "msg_1", &NOW.to_string(), body);
        assert_eq!(
            verify_webhook_at(
                &secret(b"signing-key"),
                body.as_bytes(),
                "msg_1",
                &NOW.to_string(),
                &sig,
                NOW
            ),
            Ok(())
        );
    }

    #[test]
    fn accepts_the_genuine_one_among_several() {
        let body = "{}";
        let good = sign(b"signing-key", "msg_1", &NOW.to_string(), body);
        let header = format!("v1,bm90LWl0 v1,*** {good}");
        assert_eq!(
            verify_webhook_at(
                &secret(b"signing-key"),
                body.as_bytes(),
                "msg_1",
                &NOW.to_string(),
                &header,
                NOW
            ),
            Ok(())
        );
    }

    #[test]
    fn refuses_a_tampered_body() {
        let sig = sign(
            b"signing-key",
            "msg_1",
            &NOW.to_string(),
            r#"{"type":"email.delivered"}"#,
        );
        assert_eq!(
            verify_webhook_at(
                &secret(b"signing-key"),
                br#"{"type":"email.bounced"}"#,
                "msg_1",
                &NOW.to_string(),
                &sig,
                NOW
            ),
            Err(WebhookError::SignatureMismatch)
        );
    }

    #[test]
    fn refuses_another_key_and_another_scheme() {
        let body = "{}";
        let sig = sign(b"signing-key", "msg_1", &NOW.to_string(), body);
        assert_eq!(
            verify_webhook_at(
                &secret(b"another-key"),
                body.as_bytes(),
                "msg_1",
                &NOW.to_string(),
                &sig,
                NOW
            ),
            Err(WebhookError::SignatureMismatch)
        );
        let v2 = sig.replacen("v1,", "v2,", 1);
        assert_eq!(
            verify_webhook_at(
                &secret(b"signing-key"),
                body.as_bytes(),
                "msg_1",
                &NOW.to_string(),
                &v2,
                NOW
            ),
            Err(WebhookError::SignatureMismatch)
        );
    }

    #[test]
    fn refuses_a_delivery_outside_the_window_either_side() {
        let body = "{}";
        let sig = sign(b"signing-key", "msg_1", &NOW.to_string(), body);
        for now in [NOW + TOLERANCE_SECS + 1, NOW - TOLERANCE_SECS - 1] {
            assert_eq!(
                verify_webhook_at(
                    &secret(b"signing-key"),
                    body.as_bytes(),
                    "msg_1",
                    &NOW.to_string(),
                    &sig,
                    now
                ),
                Err(WebhookError::TimestampTolerance)
            );
        }
        assert_eq!(
            verify_webhook_at(
                &secret(b"signing-key"),
                body.as_bytes(),
                "msg_1",
                "yesterday",
                &sig,
                NOW
            ),
            Err(WebhookError::InvalidTimestamp)
        );
    }

    #[test]
    fn a_secret_that_is_not_base64_is_refused_by_name() {
        assert_eq!(
            verify_webhook_at("whsec_***", b"{}", "msg_1", &NOW.to_string(), "v1,x", NOW),
            Err(WebhookError::SecretDecode)
        );
    }
}
