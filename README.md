# bjorst-resend

Thin async [Resend](https://resend.com) email client shared across bjorst
services.

The client is bound to a single sender address and posts plain-text emails to
the Resend HTTP API. Environment policy — when to enable email, which sender to
use — stays in the caller; this crate only owns the transport.

## Usage

```rust
use bjorst_resend::EmailClient;

async fn notify() -> Result<(), bjorst_resend::EmailError> {
    let client = EmailClient::new("re_123", "noreply@app.dev");
    client.send("user@example.com", "Welcome", "Thanks for signing up").await
}
```

## API

| Item                         | Behaviour                                                     |
| ---------------------------- | ------------------------------------------------------------ |
| `EmailClient::new(key, from)`| Build a client; `from` is the required sender address.       |
| `client.send(to, subj, text)`| POST a plain-text email; `EmailError` on transport/API fail. |
| `EmailError`                 | `Request` (HTTP failure) or `Api { status, body }`.          |

## License

MIT
