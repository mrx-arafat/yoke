//! A one-time launch code is the only secret printed or passed to the browser opener. The
//! browser trades it for an HttpOnly session cookie, so the session secret never appears in
//! process arguments, terminal output, URLs, browser history or page HTML.

use std::time::{Duration, Instant};

use super::{constant_eq, http::Request, query, random_token, security_headers, Console};
use anyhow::Result;
use sha2::{Digest, Sha256};

/// Long enough to paste a `--no-open` link, short enough that a stale link is useless.
const LIFETIME: Duration = Duration::from_secs(300);

const EXPIRED_STYLE: &str = ":root{color-scheme:light dark;--paper:oklch(95% .025 285);--mint:oklch(96% .022 165);--surface:oklch(100% 0 0);--well:oklch(96% .015 285);--ink:oklch(24% .035 275);--muted:oklch(43% .03 275);--line:oklch(87% .025 280)}@media(prefers-color-scheme:dark){:root{--paper:oklch(19% .025 280);--mint:oklch(21% .025 190);--surface:oklch(24% .026 280);--well:oklch(28% .03 280);--ink:oklch(96% .012 280);--muted:oklch(78% .025 280);--line:oklch(42% .035 280)}}*{box-sizing:border-box}body{margin:0;min-height:100vh;display:grid;place-items:center;padding:1rem;background:linear-gradient(115deg,var(--paper),var(--mint));color:var(--ink);font:1rem/1.5 \"Avenir Next\",\"Trebuchet MS\",sans-serif}main{max-width:36rem;padding:3rem 1.5rem;text-align:center;border:1px dashed var(--line);border-radius:1.5rem;background:var(--surface)}h1{font-weight:600;font-size:clamp(1.5rem,3vw,2rem);line-height:1.2;letter-spacing:-.035em;margin:0 0 .75rem}p{margin:0 0 .75rem;color:var(--muted)}p:last-child{margin:0}code{font:.875rem \"SFMono-Regular\",Consolas,monospace;padding:.1rem .4rem;border-radius:.4rem;background:var(--well);color:var(--ink)}";

pub(super) struct Launch {
    code: Option<String>,
    issued: Instant,
}

impl Launch {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            code: Some(random_token()?),
            issued: Instant::now(),
        })
    }

    pub(super) fn url(&self, console: &Console) -> String {
        format!(
            "http://{}{}launch?code={}",
            console.host,
            base(console),
            self.code.as_deref().unwrap_or_default()
        )
    }

    /// Spend the code; it works once and only within its lifetime.
    fn redeem(&mut self, supplied: &str) -> bool {
        let valid = self.issued.elapsed() < LIFETIME
            && self
                .code
                .as_deref()
                .is_some_and(|code| constant_eq(code, supplied));
        if valid {
            self.code = None;
        }
        valid
    }
}

/// The per-run path every console URL lives under, `/c/<32 hex>/`. It is derived one-way from
/// the session secret, so it is unguessable and changes every run without revealing the secret.
pub(super) fn base(console: &Console) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("yoke console path\n{}", console.token))
    );
    format!("/c/{}/", &digest[..32])
}

/// Cookies ignore ports, so the name carries the port to keep concurrent consoles apart.
fn cookie_name(console: &Console) -> String {
    let port = console.host.rsplit(':').next().unwrap_or_default();
    format!("yoke_{port}")
}

/// The session secret from a `Cookie` header, if this console's cookie is present.
pub(super) fn session<'a>(console: &Console, header: &'a str) -> Option<&'a str> {
    let name = cookie_name(console);
    header
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

pub(super) fn requested(console: &Console, request: &Request) -> bool {
    request.method() == "GET"
        && query(request.url()).is_ok_and(|(path, _)| path == format!("{}launch", base(console)))
}

/// Redeem the launch code for the session cookie, then send the browser to the console.
pub(super) fn respond(console: &Console, launch: &mut Launch, request: Request) -> Result<()> {
    let (_, params) = query(request.url())?;
    let host = request.header("host") == Some(console.host.as_str());
    let code = params.get("code").map(String::as_str).unwrap_or_default();
    if !host || !launch.redeem(code) {
        return expired(request);
    }
    eprintln!("Yoke console: launch link used; it no longer works.");
    let base = base(console);
    let mut headers = security_headers("text/plain; charset=utf-8");
    headers.push(("Location".into(), format!("{base}decisions")));
    // RFC 6265 8.5: cookies ignore ports, so Path keeps the secret off every other 127.0.0.1
    // server; their URLs never fall under this run's unguessable prefix.
    headers.push((
        "Set-Cookie".into(),
        format!(
            "{}={}; Path={base}; HttpOnly; SameSite=Strict",
            cookie_name(console),
            console.token
        ),
    ));
    request.respond(303, headers, b"")?;
    Ok(())
}

/// Static refusal page: no secret, no cookie, no project data.
fn expired(request: Request) -> Result<()> {
    // A hash-allowed inline style keeps /assets/console.css behind the session cookie.
    let allowed = format!(
        "style-src 'sha256-{}'",
        base64(&Sha256::digest(EXPIRED_STYLE.as_bytes()))
    );
    let mut headers = security_headers("text/html; charset=utf-8");
    for (name, value) in &mut headers {
        if name == "Content-Security-Policy" {
            *value = value.replace("style-src 'self'", &allowed);
        }
    }
    let body = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="light dark"><title>Launch link expired &middot; Yoke</title><style>{EXPIRED_STYLE}</style></head><body><main role="alert"><h1>This launch link no longer works</h1><p>Each console link opens once, within five minutes of starting the console.</p><p>Run <code>yoke console</code> again in your own terminal for a new link.</p></main></body></html>"#
    );
    request.respond(403, headers, body.as_bytes())?;
    Ok(())
}

/// Standard padded base64, as Content-Security-Policy hash sources require.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                TABLE[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn console() -> Console {
        Console {
            home: PathBuf::new(),
            project: PathBuf::new(),
            database: PathBuf::new(),
            token: "session".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:4242".into(),
        }
    }

    #[test]
    fn launch_code_works_once_within_its_lifetime_and_cookie_is_port_scoped() {
        let c = console();
        let mut launch = Launch::new().unwrap();
        let code = launch.code.clone().unwrap();
        assert!(launch
            .url(&c)
            .ends_with(&format!("{}launch?code={code}", base(&c))));
        assert!(!launch.redeem("wrong"));
        assert!(launch.redeem(&code));
        assert!(!launch.redeem(&code));
        // A host that booted less than LIFETIME ago has no earlier instant to expire against.
        if let Some(issued) = Instant::now().checked_sub(LIFETIME) {
            let mut stale = Launch::new().unwrap();
            stale.issued = issued;
            let code = stale.code.clone().unwrap();
            assert!(!stale.redeem(&code));
        }
        assert_eq!(
            session(&c, "yoke_9999=other; yoke_4242=session"),
            Some("session")
        );
        assert_eq!(session(&c, "yoke_9999=other"), None);
        for (input, encoded) in [
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), encoded);
        }
    }

    /// RFC 6265 section 5.1.4 path-match, which browsers apply before sending a cookie.
    fn path_matches(cookie_path: &str, request_path: &str) -> bool {
        request_path == cookie_path
            || request_path
                .strip_prefix(cookie_path)
                .is_some_and(|rest| cookie_path.ends_with('/') || rest.starts_with('/'))
    }

    fn get(
        server: &super::super::http::Server,
        host: &str,
        target: &str,
        cookie: &str,
    ) -> (Request, std::net::TcpStream) {
        use std::io::Write;
        let mut client = std::net::TcpStream::connect(host).unwrap();
        let cookie = if cookie.is_empty() {
            String::new()
        } else {
            format!("Cookie: {cookie}\r\n")
        };
        client
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n{cookie}\r\n").as_bytes())
            .unwrap();
        (server.recv().unwrap(), client)
    }

    fn read(mut client: std::net::TcpStream) -> String {
        use std::io::Read;
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn launch_link_sets_the_cookie_once_and_only_the_cookie_opens_pages() {
        use std::sync::{atomic::AtomicUsize, Arc};
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap();
        let database = project.join("state.db");
        let store = yoke_core::store::Store::open(&database).unwrap();
        let server = super::super::http::Server::bind_loopback().unwrap();
        let host = server.local_addr().unwrap().to_string();
        let c = Console {
            home: project.clone(),
            project,
            database,
            token: random_token().unwrap(),
            csrf: "csrf".into(),
            host: host.clone(),
        };
        let mut launch = Launch::new().unwrap();
        let target = launch.url(&c).replacen(&format!("http://{host}"), "", 1);
        let base = &target[..target.find("launch?code=").unwrap()];
        let prefix = base
            .strip_prefix("/c/")
            .and_then(|rest| rest.strip_suffix('/'));
        assert!(
            prefix.is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit())),
            "{base}"
        );
        let (request, client) = get(&server, &host, &target, "");
        assert!(requested(&c, &request));
        respond(&c, &mut launch, request).unwrap();
        let response = read(client);
        let cookie = format!("{}={}", cookie_name(&c), c.token);
        assert!(response.starts_with("HTTP/1.1 303"), "{response}");
        assert!(response.contains(&format!("Location: {base}decisions\r\n")));
        assert!(response.contains(&format!(
            "Set-Cookie: {cookie}; Path={base}; HttpOnly; SameSite=Strict\r\n"
        )));
        for other in ["/", "/index.html", "/api/approve", "/decisions", "/c/"] {
            assert!(
                !path_matches(base, other),
                "{other} would receive the cookie"
            );
        }
        assert!(path_matches(base, &format!("{base}actions")));
        let (request, client) = get(&server, &host, &target, "");
        respond(&c, &mut launch, request).unwrap();
        let response = read(client);
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.1 403"), "{response}");
        assert!(head.contains("Content-Type: text/html; charset=utf-8\r\n"));
        let hash = base64(&Sha256::digest(EXPIRED_STYLE.as_bytes()));
        assert!(head.contains(&format!("style-src 'sha256-{hash}'")));
        assert!(body.contains("<h1>This launch link no longer works</h1>"));
        assert!(body.contains("Run <code>yoke console</code> again in your own terminal"));
        let code = target.rsplit('=').next().unwrap();
        for secret in [c.token.as_str(), code, &cookie_name(&c), "Set-Cookie"] {
            assert!(!response.contains(secret), "{secret} leaked: {response}");
        }
        let mut actions = super::super::actions::Actions::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let other_run = format!("/c/{}/decisions", "0".repeat(32));
        for (target, sent, status) in [
            (format!("{base}decisions"), String::new(), "403"),
            (
                format!("{base}decisions?token={}", c.token),
                String::new(),
                "403",
            ),
            (format!("{base}decisions"), cookie.clone(), "200"),
            (format!("{base}assets/console.css"), cookie.clone(), "200"),
            ("/decisions".to_owned(), cookie.clone(), "404"),
            ("/".to_owned(), cookie.clone(), "404"),
            (other_run, cookie.clone(), "404"),
            (base.trim_end_matches('/').to_owned(), cookie.clone(), "404"),
        ] {
            let (request, client) = get(&server, &host, &target, &sent);
            super::super::serve(&c, &store, &mut actions, &connections, request).unwrap();
            let response = read(client);
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status}")),
                "{target}: {response}"
            );
            if status == "200" {
                assert!(!response.contains(&c.token));
            }
        }
    }
}
