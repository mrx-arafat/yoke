use super::Console;

pub(super) fn escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(super) fn encode(input: &str) -> String {
    input
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Page-relative: every console page sits directly under the per-run path prefix that scopes
/// the session cookie, so relative URLs stay inside it.
pub(super) fn link(_console: &Console, page: &str) -> String {
    page.to_owned()
}

/// `page`, plus `?` and the non-empty `key=value` pieces of an `&amp;`-separated query.
pub(super) fn href(page: &str, query: &str) -> String {
    let pairs: Vec<&str> = query
        .split("&amp;")
        .filter(|pair| !pair.is_empty() && !pair.ends_with('='))
        .collect();
    if pairs.is_empty() {
        page.to_owned()
    } else {
        format!("{page}?{}", pairs.join("&amp;"))
    }
}

pub(super) fn title(page: &str) -> &'static str {
    match page {
        "projects" => "Projects",
        "sessions" => "Sessions",
        "plans" => "Plans",
        "memory" => "Memory",
        "trust" => "Trust & config",
        "usage" => "Usage",
        "notifications" => "Notifications",
        "settings" => "Workflow settings",
        _ => "Decisions",
    }
}

fn nav_icon(page: &str) -> String {
    let paths = match page {
        "projects" => {
            r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"/><path d="M3 11h18"/>"#
        }
        "sessions" => {
            r#"<rect x="3" y="4" width="18" height="16" rx="4"/><path d="m7 9 3 3-3 3m6 0h4"/>"#
        }
        "plans" => {
            r#"<rect x="5" y="4" width="14" height="17" rx="3"/><path d="M9 4V2h6v2M9 10h6m-6 4h6m-6 4h3"/>"#
        }
        "memory" => {
            r#"<path d="M9 3a4 4 0 0 0-4 4v1a4 4 0 0 0-2 7 4 4 0 0 0 6 6h3V3H9Zm6 0a4 4 0 0 1 4 4v1a4 4 0 0 1 2 7 4 4 0 0 1-6 6h-3V3h3ZM7 8l5 3m-5 5 5-2m5-6-5 3m5 5-5-2"/>"#
        }
        "trust" => {
            r#"<path d="m12 2 8 3v6c0 5-4 9-8 11-4-2-8-6-8-11V5l8-3Z"/><path d="m8 12 3 3 5-6"/>"#
        }
        "usage" => r#"<path d="M4 3v17h17M8 15v-4m5 4V6m5 9V9"/>"#,
        "notifications" => {
            r#"<path d="M6 16V11a6 6 0 0 1 12 0v5l2 2H4l2-2Z"/><path d="M10 21h4"/>"#
        }
        "settings" => {
            r#"<path d="M4 6h10m4 0h2M4 12h4m4 0h8M4 18h12"/><circle cx="16" cy="6" r="2"/><circle cx="10" cy="12" r="2"/><circle cx="18" cy="18" r="2"/>"#
        }
        _ => {
            r#"<rect x="3" y="3" width="18" height="18" rx="4"/><path d="m7 8 1 1 2-2m3 1h4m-10 5 1 1 2-2m3 1h4m-10 5h10"/>"#
        }
    };
    format!(
        r#"<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{paths}</svg>"#
    )
}

pub(super) fn shell(console: &Console, page: &str, body: &str, unread: i64) -> String {
    let mut nav = String::new();
    let mut palette = String::new();
    for (name, key) in [
        ("decisions", "d"),
        ("projects", "o"),
        ("sessions", "s"),
        ("plans", "p"),
        ("memory", "m"),
        ("trust", "t"),
        ("usage", "u"),
        ("notifications", "n"),
        ("settings", "w"),
    ] {
        let current = if name == page {
            " aria-current=\"page\""
        } else {
            ""
        };
        let badge = if name == "notifications" {
            format!(
                r#"<span class="nav-count" data-unread-count{}><span data-unread-value>{unread}</span><span class="sr-only"> unread</span></span>"#,
                if unread > 0 { "" } else { " hidden" }
            )
        } else {
            String::new()
        };
        nav.push_str(&format!(
            r##"<a data-nav href="{}"{current} title="{}">{}<span>{}</span>{badge}<kbd>{key}</kbd></a>"##,
            link(console, name),
            title(name),
            nav_icon(name),
            title(name)
        ));
        palette.push_str(&format!(r##"<button type="button" data-destination="{}"><span>{}</span><kbd>g {key}</kbd></button>"##,link(console,name),title(name)));
    }
    let icon = include_str!("yoke.svg");
    format!(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="light dark"><meta name="yoke-csrf" content="{}"><meta name="description" content="Yoke local review console: decisions, plans, memory and evidence."><title>{} &middot; Yoke</title><link rel="icon" href="{}"><link rel="stylesheet" href="{}"><script defer src="{}"></script></head><body><a class="skip" href="#main">Skip to content</a><div class="app"><aside class="rail"><a class="brand" data-nav href="{}"><span class="brand-mark" aria-hidden="true">{icon}</span><span>yoke<small>Local workbench</small></span></a><nav aria-label="Workspace">{nav}</nav><div class="rail-bottom"><span class="local-mark">Local only</span><p>Your evidence stays on this machine.</p><button id="help-open" class="quiet" type="button">Keyboard shortcuts <kbd>?</kbd></button></div></aside><div class="workspace"><header class="topbar"><span class="project" title="{}">{}</span><div class="tools"><span id="connection" class="connection" role="status">Live</span><label class="theme-label"><span class="sr-only">Color theme</span><select id="theme" aria-label="Color theme"><option value="system">System</option><option value="light">Light</option><option value="dark">Dark</option></select></label><button id="palette-open" type="button" aria-label="Open command palette">Commands <kbd>Cmd K</kbd></button></div></header><div id="notice" class="notice" hidden><span>New activity is available.</span><button type="button" data-refresh>Refresh view</button></div><div id="view">{body}</div><footer class="footer"><span>Evidence before confidence.</span><span>Yoke &middot; Private by design</span></footer></div></div><div id="announcement" class="sr-only" role="status" aria-live="polite"></div><div id="toast" class="toast" role="status" hidden></div><dialog id="palette" aria-labelledby="palette-title"><div class="dialog-heading"><h2 id="palette-title">Go to your work</h2><button type="button" data-close aria-label="Close command palette">x</button></div><label class="sr-only" for="command-search">Find a page or action</label><input id="command-search" placeholder="Find a page..." autocomplete="off"><div class="command-list">{palette}</div><p class="muted">Up Down to move &middot; Enter to open &middot; Escape to close</p></dialog><dialog id="help" aria-labelledby="help-title"><div class="dialog-heading"><h2 id="help-title">Keep your hands on the keyboard</h2><button type="button" data-close aria-label="Close shortcuts">x</button></div><dl class="shortcuts"><dt>Command palette</dt><dd><kbd>Cmd / Ctrl K</kbd></dd><dt>Move through records</dt><dd><kbd>j</kbd> <kbd>k</kbd></dd><dt>Open focused record</dt><dd><kbd>Enter</kbd></dd><dt>Go to page</dt><dd><kbd>g</kbd> then <kbd>d / o / s / p / m / t / u / n / w</kbd></dd><dt>Close a dialog</dt><dd><kbd>Esc</kbd></dd></dl></dialog><dialog id="lightbox" aria-label="Check screenshot"><div class="dialog-heading"><p id="image-caption"></p><button type="button" data-close aria-label="Close screenshot">x</button></div><img id="lightbox-image" alt="Check evidence screenshot"><div class="actions"><button type="button" id="image-prev">Previous</button><button type="button" id="image-next">Next</button></div></dialog></body></html>"##,
        console.csrf,
        title(page),
        link(console, "icon.svg"),
        link(console, "assets/console.css"),
        link(console, "assets/console.js"),
        link(console, "decisions"),
        escape(&console.project.display().to_string()),
        escape(
            console
                .project
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Workspace")
        )
    )
}

pub(super) fn heading(page: &str, description: &str) -> String {
    format!(
        r##"<main id="main" tabindex="-1" data-page="{page}"><div class="page-heading"><div><p class="kicker">Workspace / {}</p><h1>{}</h1><p class="lede">{}</p></div><span class="section-index" aria-hidden="true">{}</span></div>"##,
        title(page),
        title(page),
        escape(description),
        match page {
            "decisions" => "->",
            "plans" => "<>",
            "memory" => "...",
            _ => "&middot;",
        }
    )
}

pub(super) fn empty(title: &str, description: &str, command: &str) -> String {
    format!(
        r##"<section class="empty"><div class="empty-mark" aria-hidden="true">-></div><h2>{}</h2><p>{}</p><code>{}</code></section>"##,
        escape(title),
        escape(description),
        escape(command)
    )
}

pub(super) fn error(message: &str) -> String {
    format!(
        r##"<main id="main" tabindex="-1" data-page="error"><section class="empty error" role="alert"><h1>This view couldn't load</h1><p>{}</p><button type="button" data-refresh>Try again</button></section></main>"##,
        escape(message)
    )
}
