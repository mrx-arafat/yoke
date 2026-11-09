//! Local-only human review console. Authentication tokens live only in memory.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicUsize, Arc, Mutex, PoisonError};

use self::http::{Request, Server};
use anyhow::{ensure, Context, Result};
use yoke_core::store::Store;

#[path = "console/actions.rs"]
mod actions;
#[path = "console/changes.rs"]
mod changes;
#[path = "console/content.rs"]
mod content;
#[path = "console/dashboard.rs"]
mod dashboard;
#[path = "console/docs.rs"]
mod docs;
#[path = "console/events.rs"]
mod events;
#[path = "console/extensions.rs"]
mod extensions;
#[path = "console/http.rs"]
mod http;
#[path = "console/insights.rs"]
mod insights;
#[path = "console/launch.rs"]
mod launch;
#[path = "console/notifications.rs"]
mod notifications;
#[path = "console/pages.rs"]
mod pages;
#[path = "console/plan_detail.rs"]
mod plan_detail;
#[path = "console/policy.rs"]
mod policy;
#[path = "console/projects.rs"]
mod projects;
#[path = "console/render.rs"]
mod render;
#[path = "console/requirements.rs"]
mod requirements;
#[path = "console/settings.rs"]
mod settings;
#[path = "console/system.rs"]
mod system;
#[path = "console/usage.rs"]
mod usage;
#[path = "console/views.rs"]
mod views;

pub(super) struct Console {
    home: PathBuf,
    project: PathBuf,
    database: PathBuf,
    token: String,
    csrf: String,
    host: String,
}

/// A view error caused by the request itself; it renders with HTTP 400 instead of 500.
#[derive(Debug)]
pub(super) struct BadRequest(pub(super) String);

impl std::fmt::Display for BadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BadRequest {}

fn view_status(error: &anyhow::Error) -> u16 {
    if error.downcast_ref::<policy::MissingVersion>().is_some() {
        404
    } else if error.downcast_ref::<BadRequest>().is_some() {
        400
    } else {
        500
    }
}

pub(super) fn random_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Serve the explicit home's state on loopback until the process is stopped.
/// `port` 0 picks a free port; a fixed port lets an SSH tunnel be set up before the console starts.
pub fn run(home: &Path, project: &Path, open_browser: bool, port: u16) -> Result<()> {
    ensure!(
        home.is_absolute() && project.is_absolute(),
        "console paths must be absolute"
    );
    let project = project.canonicalize().context("resolve console project")?;
    let state = home.join(".yoke");
    std::fs::create_dir_all(&state)?;
    let database = state.join("state.db");
    // Open once up front so a broken or newer database fails before the link is printed;
    // each request then opens its own connection.
    Store::open(&database)?;
    let server = Server::bind_loopback_on(port).with_context(|| {
        format!("listen on 127.0.0.1:{port}; another process may already use that port")
    })?;
    let address = server.local_addr()?;
    let host = address.to_string();
    let console = Console {
        home: home.to_owned(),
        project,
        database,
        token: random_token()?,
        csrf: random_token()?,
        host,
    };
    let launch = launch::Launch::new()?;
    let url = launch.url(&console);
    println!("Yoke console: {url}");
    eprintln!("This link works once, within five minutes, and signs that browser in.");
    let env = |name: &str| std::env::var(name).ok();
    if open_browser {
        if headless_ssh(env) {
            eprintln!("This SSH session has no display, so no browser was started; open the link yourself.");
        } else {
            let command = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            if let Some(note) = open_link(command, &url) {
                eprintln!("Could not open a browser ({note}); the console is still running, so open the link above yourself.");
            }
        }
    }
    if env("SSH_CONNECTION").is_some() {
        eprintln!("{}", tunnel_hint(address.port()));
    }
    let console = Arc::new(console);
    let actions = Arc::new(actions::Actions::default());
    let launch = Arc::new(Mutex::new(launch));
    let connections = Arc::new(AtomicUsize::new(0));
    loop {
        let request = server.recv()?;
        let (console, actions, launch, connections) = (
            Arc::clone(&console),
            Arc::clone(&actions),
            Arc::clone(&launch),
            Arc::clone(&connections),
        );
        // One thread and one database connection per request, so a slow action (a push, a
        // reviewer CLI) never stalls page loads. The server already caps open connections.
        std::thread::spawn(move || {
            let result = if launch::requested(&console, &request) {
                let mut launch = launch.lock().unwrap_or_else(PoisonError::into_inner);
                launch::respond(&console, &mut launch, request)
            } else {
                match Store::open(&console.database) {
                    Ok(store) => serve(&console, &store, &actions, &connections, request),
                    Err(error) => respond(
                        request,
                        503,
                        "text/plain; charset=utf-8",
                        "The Yoke state database could not be opened; retry, and run yoke health in a terminal if it persists",
                    )
                    .and(Err(error)),
                }
            };
            if let Err(error) = result {
                let message = yoke_core::redact::redact(&error.to_string());
                system::record_console_error(&message);
                eprintln!("yoke console: {message}");
            }
        });
    }
}

/// An SSH login with no display cannot show a browser, and `xdg-open` may start a text browser
/// inside the console's own terminal.
fn headless_ssh(env: impl Fn(&str) -> Option<String>) -> bool {
    let unset = |name: &str| env(name).is_none_or(|value| value.is_empty());
    env("SSH_CONNECTION").is_some() && unset("DISPLAY") && unset("WAYLAND_DISPLAY")
}

/// Ask the desktop to open the link; the result is a note for the user, never a reason to stop.
fn open_link(command: &str, url: &str) -> Option<String> {
    match std::process::Command::new(command).arg(url).status() {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!("{command} exited with {status}")),
        Err(error) => Some(format!("{command}: {error}")),
    }
}

/// The browser's `Host` must equal the console's own address, so both ports must match.
fn tunnel_hint(port: u16) -> String {
    format!("Over SSH? On the computer with the browser run `ssh -N -L {port}:127.0.0.1:{port} <this server>`, then open the link above unchanged (keep 127.0.0.1, not localhost). `yoke console --port <n>` fixes the port so the tunnel can come first.")
}

fn header(request: &Request, name: &str) -> Option<String> {
    request.header(name).map(str::to_owned)
}

fn constant_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// Apply to bootstrap and authenticated reads as well as actions: a foreign website must
/// never spend a launch code or use an ambient cookie, even without Fetch Metadata.
fn same_site_request(console: &Console, header: impl Fn(&str) -> Option<String>) -> bool {
    header("host").as_deref() == Some(console.host.as_str())
        && header("origin").is_none_or(|value| value == format!("http://{}", console.host))
        && header("sec-fetch-site").is_none_or(|value| value == "same-origin" || value == "none")
}

/// Only the HttpOnly session cookie authenticates; URLs and headers never carry the secret.
fn authorize(console: &Console, method: &str, headers: &BTreeMap<String, String>) -> bool {
    let supplied = headers
        .get("cookie")
        .and_then(|value| launch::session(console, value))
        .unwrap_or("");
    if !constant_eq(supplied, &console.token)
        || !same_site_request(console, |name| headers.get(name).cloned())
    {
        return false;
    }
    if method == "POST" {
        return headers.get("origin") == Some(&format!("http://{}", console.host))
            && headers
                .get("x-yoke-csrf")
                .is_some_and(|v| constant_eq(v, &console.csrf))
            && headers
                .get("content-type")
                .is_some_and(|v| v.split(';').next() == Some("application/json"));
    }
    method == "GET"
}

fn decode(input: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut iter = input.bytes();
    while let Some(byte) = iter.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let a = iter
                    .next()
                    .and_then(|v| (v as char).to_digit(16))
                    .context("invalid percent encoding")?;
                let b = iter
                    .next()
                    .and_then(|v| (v as char).to_digit(16))
                    .context("invalid percent encoding")?;
                bytes.push((a * 16 + b) as u8);
            }
            _ => bytes.push(byte),
        }
    }
    Ok(String::from_utf8(bytes)?)
}

pub(super) fn query(url: &str) -> Result<(&str, BTreeMap<String, String>)> {
    ensure!(url.len() <= 8192, "request URL too long");
    let (path, raw) = url.split_once('?').unwrap_or((url, ""));
    let mut result = BTreeMap::new();
    for part in raw.split('&').filter(|s| !s.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        ensure!(
            result.insert(decode(key)?, decode(value)?).is_none(),
            "duplicate query parameter"
        );
    }
    Ok((path, result))
}

fn serve(
    console: &Console,
    store: &Store,
    actions: &actions::Actions,
    connections: &Arc<AtomicUsize>,
    request: Request,
) -> Result<()> {
    let headers: BTreeMap<String, String> = [
        "cookie",
        "host",
        "origin",
        "x-yoke-csrf",
        "content-type",
        "sec-fetch-site",
    ]
    .into_iter()
    .filter_map(|name| header(&request, name).map(|v| (name.to_owned(), v)))
    .collect();
    let url = request.url().to_owned();
    let (path, params) = match query(&url) {
        Ok(value) => value,
        Err(_) => {
            return respond(
                request,
                400,
                "text/plain; charset=utf-8",
                "Invalid request URL; reopen this page from the console navigation without modifying its query parameters",
            )
        }
    };
    // Everything lives under the per-run prefix that scopes the session cookie.
    let base = launch::base(console);
    let Some(path) = path
        .strip_prefix(&base[..base.len() - 1])
        .filter(|rest| rest.starts_with('/'))
    else {
        return respond(request, 404, "text/plain; charset=utf-8", "Page not found");
    };
    if !authorize(console, request.method(), &headers) {
        return respond(request, 403, "text/plain; charset=utf-8", "Forbidden; run yoke console in your own terminal and use the current session's browser tab");
    }
    if request.method() == "POST" {
        if path != "/actions" {
            return respond(request, 404, "text/plain", "Unknown action endpoint");
        }
        let result = (|| {
            ensure!(request.body().len() <= 16384, "Action request too large");
            let body = std::str::from_utf8(request.body()).context("Action body must be UTF-8")?;
            actions.apply(store, console, serde_json::from_str(body)?)
        })();
        return match result {
            Ok(value)=>respond(request,200,"application/json",&value.to_string()),
            Err(error)=>respond(request,409,"application/json",&serde_json::json!({"error":{"code":"action_failed","message":yoke_core::redact::redact(&format!("{error}; refresh this view and retry using its current controls"))}}).to_string()),
        };
    }
    if let Some((_, kind, body)) = PAGE_ASSETS.iter().find(|(asset, ..)| *asset == path) {
        return respond(request, 200, kind, body);
    }
    match path {
        "/assets/console.css" => respond(
            request,
            200,
            "text/css; charset=utf-8",
            include_str!("console/console.css"),
        ),
        "/assets/console.js" => respond(
            request,
            200,
            "text/javascript; charset=utf-8",
            include_str!("console/console.js"),
        ),
        "/icon.svg" => respond(
            request,
            200,
            "image/svg+xml",
            include_str!("console/yoke.svg"),
        ),
        "/events" => {
            events::start(request, console.database.clone(), Arc::clone(connections));
            Ok(())
        }
        "/artifact" => match actions::artifact(store, console, &params) {
            Ok((bytes, kind)) => {
                request.respond(200, security_headers(kind), &bytes)?;
                Ok(())
            }
            Err(error) => respond(
                request,
                404,
                "text/plain",
                &yoke_core::redact::redact(&format!("{error}; reopen the screenshot from Plans; if it is still unavailable, regenerate its project-local PNG, JPEG or WebP file under 10 MiB and rerun the matching yoke plan check <slug> --agent <claude|codex>, then refresh Plans")),
            ),
        },
        "/system/download" => match system::download(store, console, &params) {
            Ok(download) => download.send(request),
            Err(error) => respond(
                request,
                if error.downcast_ref::<BadRequest>().is_some() {
                    400
                } else {
                    404
                },
                "text/plain; charset=utf-8",
                &yoke_core::redact::redact(&format!(
                    "{error}; reopen System and choose the backup or export again"
                )),
            ),
        },
        "/" | "/decisions" | "/projects" | "/sessions" | "/plans" | "/memory" | "/trust"
        | "/usage" | "/notifications" | "/settings" | "/policy" | "/dashboard" | "/changes"
        | "/requirements" | "/docs" | "/extensions" | "/system" => {
            let page = if path == "/" { "dashboard" } else { &path[1..] };
            let result = match page {
                "dashboard" => dashboard::page(store, console, &params),
                "changes" => changes::page(store, console, &params),
                "requirements" => requirements::page(store, console, &params),
                "docs" => docs::page(store, console, &params),
                "extensions" => extensions::page(store, console, &params),
                "system" => system::page(store, console, &params),
                "notifications" => notifications::page(store, console, &params),
                "settings" => settings::page(store, console),
                "projects" => projects::overview(store, console),
                "sessions" => views::sessions(store, console, &params),
                "plans" => views::plans(store, console, &params),
                "memory" => views::memories(store, console, &params),
                "trust" => views::trust(store, console),
                "policy" => policy::page(store, console, &params),
                "usage" => usage::page(store, console, &params),
                _ => pages::decisions(store, console, &params),
            };
            let (body, status) = match result {
                Ok(body) => (body, 200),
                Err(error) => (
                    render::error(&yoke_core::redact::redact(&error.to_string())),
                    view_status(&error),
                ),
            };
            let unread = notifications::unread(store, console);
            let html = if header(&request, "x-yoke-fragment").as_deref() == Some("1") {
                format!("{body}<span hidden data-unread-update=\"{unread}\"></span>")
            } else {
                render::shell(console, page, &body, unread)
            };
            respond(request, status, "text/html; charset=utf-8", &html)
        }
        _ => respond(request, 404, "text/plain; charset=utf-8", "Page not found"),
    }
}

/// Per-page scripts and styles, each owned by one page; `render::shell` links every one.
const PAGE_ASSETS: &[(&str, &str, &str)] = &[
    (
        "/assets/changes.js",
        "text/javascript; charset=utf-8",
        include_str!("console/changes.js"),
    ),
    (
        "/assets/changes.css",
        "text/css; charset=utf-8",
        include_str!("console/changes.css"),
    ),
    (
        "/assets/plans.js",
        "text/javascript; charset=utf-8",
        include_str!("console/plans.js"),
    ),
    (
        "/assets/plans.css",
        "text/css; charset=utf-8",
        include_str!("console/plans.css"),
    ),
    (
        "/assets/extensions.js",
        "text/javascript; charset=utf-8",
        include_str!("console/extensions.js"),
    ),
    (
        "/assets/extensions.css",
        "text/css; charset=utf-8",
        include_str!("console/extensions.css"),
    ),
    (
        "/assets/memory.js",
        "text/javascript; charset=utf-8",
        include_str!("console/memory.js"),
    ),
    (
        "/assets/memory.css",
        "text/css; charset=utf-8",
        include_str!("console/memory.css"),
    ),
    (
        "/assets/system.js",
        "text/javascript; charset=utf-8",
        include_str!("console/system.js"),
    ),
    (
        "/assets/system.css",
        "text/css; charset=utf-8",
        include_str!("console/system.css"),
    ),
    (
        "/assets/settings.js",
        "text/javascript; charset=utf-8",
        include_str!("console/settings.js"),
    ),
    (
        "/assets/settings.css",
        "text/css; charset=utf-8",
        include_str!("console/settings.css"),
    ),
    (
        "/assets/sessions.js",
        "text/javascript; charset=utf-8",
        include_str!("console/sessions.js"),
    ),
    (
        "/assets/sessions.css",
        "text/css; charset=utf-8",
        include_str!("console/sessions.css"),
    ),
];

fn security_headers(kind: &str) -> Vec<(String, String)> {
    [
        ("Content-Type", kind),
        ("Cache-Control", "no-store"),
        ("Referrer-Policy", "no-referrer"),
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("Content-Security-Policy", "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"),
        ("Permissions-Policy", "camera=(), microphone=(), geolocation=()"),
    ].into_iter().map(|(name,value)| (name.to_owned(),value.to_owned())).collect()
}

fn respond(request: Request, status: u16, kind: &str, body: &str) -> Result<()> {
    request.respond(status, security_headers(kind), body.as_bytes())?;
    Ok(())
}

#[cfg(test)]
#[path = "console/tests.rs"]
mod tests;
