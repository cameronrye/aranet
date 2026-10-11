//! Redaction for text that reaches the logs from the integrations: sink URLs
//! and the response bodies of the endpoints behind them.
//!
//! Webhook URLs often carry their secret in the path (Slack, Discord) or are
//! the secret (an ntfy topic), and any sink URL can carry `user:password@`.
//! Log a sink URL only through [`redact_url`], and a response body only
//! through [`sanitize_body`]. Read the body of a sink's error response only
//! with [`read_error_body`], which also limits how much of it is read.

/// The most characters of a response body kept in an error or log line.
pub(crate) const MAX_LOGGED_BODY_CHARS: usize = 256;

/// The most bytes of an error response's body that [`read_error_body`] reads.
/// More than [`MAX_LOGGED_BODY_CHARS`] characters of any encoding.
const MAX_ERROR_BODY_BYTES: usize = 4096;

/// What a sink reports in place of the body of a 3xx answer.
pub(crate) const REDIRECT_NOT_FOLLOWED: &str =
    "redirect not followed; set the URL to its final address";

/// `scheme://host[:port]` of `url`, followed by `/…` when it had a path other
/// than `/`, a query or a fragment.
///
/// Userinfo (`user:password@`) is always dropped, and the port appears only
/// when it isn't the scheme's default. Anything that isn't an absolute URL
/// with a host gives `<invalid URL>`.
pub(crate) fn redact_url(url: &str) -> String {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return "<invalid URL>".to_string();
    };
    let Some(host) = parsed.host_str() else {
        return "<invalid URL>".to_string();
    };
    let port = parsed
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let elided = if matches!(parsed.path(), "" | "/")
        && parsed.query().is_none()
        && parsed.fragment().is_none()
    {
        ""
    } else {
        "/…"
    };
    format!("{}://{host}{port}{elided}", parsed.scheme())
}

/// `body`, fit for one log line: the characters that could split the line or
/// change how it reads (see [`is_log_breaking_char`]) become spaces, and
/// everything after the first [`MAX_LOGGED_BODY_CHARS`] characters is
/// replaced by `…`.
pub(crate) fn sanitize_body(body: &str) -> String {
    let mut chars = body.chars();
    let mut sanitized: String = chars
        .by_ref()
        .take(MAX_LOGGED_BODY_CHARS)
        .map(|c| if is_log_breaking_char(c) { ' ' } else { c })
        .collect();
    if chars.next().is_some() {
        sanitized.push('…');
    }
    sanitized
}

/// The body of a sink's error `response`, fit for one log line (see
/// [`sanitize_body`]).
///
/// The endpoint decides how long its answer is, so only the first
/// [`MAX_ERROR_BODY_BYTES`] bytes are read. The rest is never received: the
/// connection is closed instead. A body that ends early, or fails part-way,
/// gives what had arrived.
///
/// For a 3xx answer nothing is read and the result is
/// [`REDIRECT_NOT_FOLLOWED`]. The sinks' client never follows a redirect (see
/// `sink_client`), and the `Location` header never reaches a log: it is a URL
/// like the sink's own, which may hold a secret.
pub(crate) async fn read_error_body(mut response: reqwest::Response) -> String {
    if response.status().is_redirection() {
        return REDIRECT_NOT_FOLLOWED.to_string();
    }
    let mut bytes = Vec::new();
    while bytes.len() < MAX_ERROR_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let wanted = MAX_ERROR_BODY_BYTES - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(wanted)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    sanitize_body(&String::from_utf8_lossy(&bytes))
}

/// Whether `c` could split a log line or change how it reads: the control
/// characters (`char::is_control`, such as a newline or the escape that
/// starts a terminal colour sequence), the line and paragraph separators
/// U+2028/U+2029, and the bidi embedding, override and isolate controls
/// U+202A-U+202E and U+2066-U+2069. Device addresses and aliases may not
/// contain these characters either (`is_forbidden_text_char` in `config.rs`).
fn is_log_breaking_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_table() {
        let cases = [
            (
                "https://hooks.slack.com/services/T0/B0/XXXX",
                "https://hooks.slack.com/…",
            ),
            (
                "https://user:pw@push.example.com:9091/metrics/job/x",
                "https://push.example.com:9091/…",
            ),
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("http://127.0.0.1:8080/", "http://127.0.0.1:8080"),
            ("https://ntfy.sh/topic?auth=abc", "https://ntfy.sh/…"),
            ("not a url", "<invalid URL>"),
            ("http://user:pw@influx:8086", "http://influx:8086"),
            ("https://example.com/?token=abc", "https://example.com/…"),
            ("https://example.com/#token=abc", "https://example.com/…"),
            ("https://example.com:443/hook", "https://example.com/…"),
            ("http://[::1]:9091/metrics", "http://[::1]:9091/…"),
            ("mailto:alerts@example.com", "<invalid URL>"),
            ("", "<invalid URL>"),
        ];
        for (url, expected) in cases {
            assert_eq!(redact_url(url), expected, "redact_url({url:?})");
        }
    }

    #[test]
    fn sanitize_body_truncates_and_strips_controls() {
        assert_eq!(sanitize_body(""), "");
        assert_eq!(sanitize_body("invalid token"), "invalid token");
        // A newline can't start a forged log line, and an escape sequence
        // can't recolour the terminal.
        assert_eq!(
            sanitize_body("bad\r\nrequest\t\u{1b}[31m!"),
            "bad  request  [31m!"
        );
        // Nor can a Unicode line separator, and a bidi override can't make
        // the rest of the line read differently.
        assert_eq!(sanitize_body("a\u{2028}b\u{202e}c\u{2066}d"), "a b c d");

        let exact = "x".repeat(MAX_LOGGED_BODY_CHARS);
        assert_eq!(sanitize_body(&exact), exact);

        // Counted in characters, not bytes: a cut at byte 256 would split a
        // two-byte `é`.
        let long = "é".repeat(MAX_LOGGED_BODY_CHARS + 1);
        assert_eq!(
            sanitize_body(&long),
            format!("{}…", "é".repeat(MAX_LOGGED_BODY_CHARS))
        );
    }

    #[test]
    fn log_breaking_chars_are_exactly_the_documented_set() {
        // Both ends of every range that `is_log_breaking_char`'s doc lists
        // (`char::is_control` is U+0000-U+001F and U+007F-U+009F), and its two
        // single code points.
        for c in [
            '\u{0}', '\u{1f}', '\u{7f}', '\u{9f}', '\u{2028}', '\u{2029}', '\u{202a}', '\u{202e}',
            '\u{2066}', '\u{2069}',
        ] {
            assert!(
                is_log_breaking_char(c),
                "U+{:04X} must be replaced",
                u32::from(c)
            );
        }

        // The nearest code points outside each range are kept.
        for c in [
            ' ', '~', '\u{a0}', '\u{2027}', '\u{202f}', '\u{2065}', '\u{206a}',
        ] {
            assert!(
                !is_log_breaking_char(c),
                "U+{:04X} must be kept",
                u32::from(c)
            );
        }

        // Nothing in between differs either: a code point is replaced exactly
        // when the doc lists it.
        for c in '\0'..=char::MAX {
            let documented = matches!(
                u32::from(c),
                0x00..=0x1f | 0x7f..=0x9f | 0x2028 | 0x2029 | 0x202a..=0x202e | 0x2066..=0x2069
            );
            assert_eq!(
                is_log_breaking_char(c),
                documented,
                "U+{:04X}",
                u32::from(c)
            );
        }
    }
}
