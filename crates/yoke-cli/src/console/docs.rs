//! Documentation: Yoke's own docs, embedded at build time (see `build.rs`).
//! Owner: parity-plans (Pilot parity fleet). Read-only, so it has no actions.
use super::{
    content,
    render::{self, encode, escape},
    BadRequest, Console,
};
use anyhow::Result;
use pulldown_cmark::{html, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};
use std::collections::BTreeMap;
use yoke_core::store::Store;

include!(concat!(env!("OUT_DIR"), "/embedded_docs.rs"));

/// Pages shown first, in this order; the rest follow alphabetically.
const FIRST: [&str; 6] = [
    "manual",
    "getting-started",
    "user-guide",
    "cli",
    "configuration",
    "troubleshooting",
];

struct Page {
    slug: String,
    /// Repository-relative path, for resolving links between pages.
    path: &'static str,
    source: &'static str,
    title: String,
}

fn pages() -> Vec<Page> {
    let mut pages: Vec<Page> = EMBEDDED
        .iter()
        .map(|(path, source)| {
            let slug = if *path == "MANUAL.md" {
                "manual".to_owned()
            } else {
                path.trim_start_matches("docs/")
                    .trim_end_matches(".md")
                    .to_owned()
            };
            let title = source
                .lines()
                .find_map(|line| line.strip_prefix("# "))
                .map_or_else(|| slug.clone(), |t| t.trim().to_owned());
            Page {
                slug,
                path,
                source,
                title,
            }
        })
        .collect();
    pages.sort_by_key(|p| {
        (
            FIRST
                .iter()
                .position(|f| *f == p.slug)
                .unwrap_or(FIRST.len()),
            p.slug.clone(),
        )
    });
    pages
}

struct Heading {
    level: u8,
    text: String,
    id: String,
}

/// GitHub-style anchor: lower case, letters, digits, `-` and `_` kept, spaces become `-`.
fn slugify(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// `docs/a/../b.md` style paths resolved against the directory of `from`.
fn resolve_path(from: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// What a link start emitted, so its end closes exactly that.
enum Open {
    Anchor,
    Show(String),
    Silent,
}

enum Target {
    /// A page of this console, possibly with an anchor.
    Page(String),
    Anchor(String),
    /// Shown as text with its address, never as a link.
    External(String),
    Dropped,
}

fn classify(from: &str, dest: &str, pages: &[Page]) -> Target {
    if let Some(fragment) = dest.strip_prefix('#') {
        return Target::Anchor(format!("#doc-{}", slugify(fragment)));
    }
    if dest.starts_with("http://") || dest.starts_with("https://") {
        return Target::External(dest.to_owned());
    }
    let (path, fragment) = dest.split_once('#').unwrap_or((dest, ""));
    if path.contains(':') {
        return Target::Dropped;
    }
    let Some(resolved) = resolve_path(from, path) else {
        return Target::Dropped;
    };
    match pages.iter().find(|p| p.path == resolved) {
        Some(page) => {
            let mut href = render::href("docs", &format!("page={}", encode(&page.slug)));
            if !fragment.is_empty() {
                href.push_str(&format!("#doc-{}", slugify(fragment)));
            }
            Target::Page(href)
        }
        None => Target::Dropped,
    }
}

/// Markdown to safe HTML: no raw HTML, no images, headings get anchors, and only links to other
/// embedded pages or to anchors are real links. Web addresses stay readable as text.
fn render_markdown(from: &str, source: &str, pages: &[Page]) -> (String, Vec<Heading>) {
    let events: Vec<Event> = Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    )
    .collect();
    let mut out: Vec<Event> = Vec::new();
    let mut headings: Vec<Heading> = Vec::new();
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    let mut code: Option<String> = None;
    let mut links: Vec<Open> = Vec::new();
    let mut index = 0;
    while index < events.len() {
        let event = events[index].clone();
        index += 1;
        match event {
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(code) = code.take() {
                    out.push(Event::Html(
                        format!("<pre><code>{}</code></pre>", content::highlight(&code)).into(),
                    ));
                }
            }
            Event::Text(text) if code.is_some() => code.as_mut().unwrap().push_str(&text),
            Event::Html(text) | Event::InlineHtml(text) => out.push(Event::Text(text)),
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => match classify(from, &dest_url, pages) {
                Target::Page(href) => {
                    links.push(Open::Anchor);
                    out.push(Event::Html(
                        format!("<a data-nav href=\"{}\">", escape(&href)).into(),
                    ));
                }
                Target::Anchor(href) => {
                    links.push(Open::Anchor);
                    out.push(Event::Html(
                        format!("<a href=\"{}\">", escape(&href)).into(),
                    ));
                }
                Target::External(url) => {
                    // An autolink already shows its address.
                    let show = !matches!(link_type, LinkType::Autolink | LinkType::Email);
                    links.push(if show { Open::Show(url) } else { Open::Silent });
                }
                Target::Dropped => links.push(Open::Silent),
            },
            Event::End(TagEnd::Link) => match links.pop() {
                Some(Open::Anchor) => out.push(Event::Html("</a>".into())),
                Some(Open::Show(url)) => out.push(Event::Text(format!(" ({url})").into())),
                _ => {}
            },
            Event::Start(Tag::Heading { level, .. }) => {
                let mut text = String::new();
                let mut look = index;
                while look < events.len() {
                    match &events[look] {
                        Event::End(TagEnd::Heading(_)) => break,
                        Event::Text(t) | Event::Code(t) => text.push_str(t),
                        _ => {}
                    }
                    look += 1;
                }
                let base = slugify(&text);
                let count = used.entry(base.clone()).or_default();
                let id = if *count == 0 {
                    base.clone()
                } else {
                    format!("{base}-{count}")
                };
                *count += 1;
                let shown = match level {
                    HeadingLevel::H1 => 2,
                    HeadingLevel::H2 => 3,
                    _ => 4,
                };
                if !matches!(level, HeadingLevel::H1) {
                    headings.push(Heading {
                        level: level as u8,
                        text: text.clone(),
                        id: id.clone(),
                    });
                }
                out.push(Event::Html(
                    format!("<h{shown} id=\"doc-{}\">", escape(&id)).into(),
                ));
            }
            Event::End(TagEnd::Heading(level)) => {
                let shown = match level {
                    HeadingLevel::H1 => 2,
                    HeadingLevel::H2 => 3,
                    _ => 4,
                };
                out.push(Event::Html(format!("</h{shown}>").into()));
            }
            other => out.push(other),
        }
    }
    let mut html_out = String::new();
    html::push_html(&mut html_out, out.into_iter());
    (html_out, headings)
}

fn summary(source: &str) -> String {
    let paragraph = source
        .split("\n\n")
        .map(str::trim)
        .find(|block| {
            !block.is_empty()
                && !block.starts_with('#')
                && !block.starts_with("```")
                && !block.starts_with('|')
                && !block.starts_with('-')
        })
        .unwrap_or("");
    let flat: String = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = flat.chars().take(180).collect();
    if flat.chars().count() > 180 {
        format!("{text}...")
    } else {
        text
    }
}

fn index(pages: &[Page]) -> String {
    let mut out = render::heading("docs", "Yoke's manual and reference, offline.");
    if pages.is_empty() {
        out.push_str(&render::empty(
            "Documentation is not part of this build",
            "This binary was built without the repository's docs folder. Read the same pages in the Yoke repository under MANUAL.md and docs/.",
            "yoke --help",
        ));
    }
    for page in pages {
        out.push_str(&format!(
            r#"<article class="panel"><div class="list-caption"><h2><a data-nav data-row href="{}">{}</a></h2><span class="muted">{} KiB</span></div><p class="muted">{}</p></article>"#,
            render::href("docs", &format!("page={}", encode(&page.slug))),
            escape(&page.title),
            page.source.len().div_ceil(1024),
            escape(&summary(page.source))
        ));
    }
    out.push_str("<p class=\"muted\">These pages are built into this Yoke binary and need no network. Links to the web are shown as plain text.</p></main>");
    out
}

fn page_view(c: &Console, pages: &[Page], slug: &str) -> Result<String> {
    let Some(page) = pages.iter().find(|p| p.slug == slug) else {
        return Err(BadRequest(
            "Unknown documentation page. Choose one from the Documentation list.".into(),
        )
        .into());
    };
    let (body, headings) = render_markdown(page.path, page.source, pages);
    let mut out = render::heading("docs", "Yoke's manual and reference, offline.");
    out.push_str(&format!(
        r#"<p><a data-nav href="{}">All documentation</a></p><nav class="doc-pages" aria-label="Documentation pages">"#,
        render::link(c, "docs")
    ));
    for other in pages {
        out.push_str(&format!(
            r#"<a data-nav href="{}"{}>{}</a>"#,
            render::href("docs", &format!("page={}", encode(&other.slug))),
            if other.slug == slug {
                " aria-current=\"page\""
            } else {
                ""
            },
            escape(&other.slug)
        ));
    }
    out.push_str("</nav>");
    if !headings.is_empty() {
        out.push_str(
            "<nav class=\"panel doc-toc\" aria-label=\"On this page\"><h2>On this page</h2><ul>",
        );
        for heading in headings.iter().filter(|h| h.level <= 3) {
            out.push_str(&format!(
                r##"<li class="toc-{}"><a href="#doc-{}">{}</a></li>"##,
                heading.level,
                escape(&heading.id),
                escape(&heading.text)
            ));
        }
        out.push_str("</ul></nav>");
    }
    out.push_str(&format!(
        "<article class=\"panel prose doc-body\">{body}</article></main>"
    ));
    Ok(out)
}

pub(super) fn page(
    _store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let pages = pages();
    match params.get("page") {
        Some(slug) => page_view(c, &pages, slug),
        None => Ok(index(&pages)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Page> {
        vec![
            Page {
                slug: "manual".into(),
                path: "MANUAL.md",
                source: "# Manual",
                title: "Manual".into(),
            },
            Page {
                slug: "cli".into(),
                path: "docs/cli.md",
                source: "# CLI",
                title: "CLI".into(),
            },
        ]
    }

    #[test]
    fn links_resolve_between_pages_and_nothing_else_becomes_a_link() {
        let source = "# Title\n\n## Setup steps\n\n## Setup steps\n\n[cli](cli.md#flags) [up](../MANUAL.md) [self](#setup-steps) [web](https://example.com/x) [js](javascript:alert(1)) [gone](missing.md)\n\n<script>x</script>\n\n![img](http://e/x.png)";
        let (html, headings) = render_markdown("docs/user-guide.md", source, &fixture());

        assert!(
            html.contains(r#"<a data-nav href="docs?page=cli#doc-flags">cli</a>"#),
            "{html}"
        );
        assert!(html.contains(r#"<a data-nav href="docs?page=manual">up</a>"#));
        assert!(html.contains(r##"<a href="#doc-setup-steps">self</a>"##));
        assert!(html.contains("web (https://example.com/x)") && !html.contains("example.com/x\""));
        assert!(
            !html.contains("javascript") && !html.contains("<script") && !html.contains("<img")
        );
        assert!(html.contains("gone") && !html.contains("missing.md"));
        assert_eq!(
            headings.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            ["setup-steps", "setup-steps-1"],
            "repeated headings get distinct anchors"
        );
    }

    #[test]
    fn every_embedded_page_renders_and_the_manual_links_to_its_siblings() {
        let pages = pages();
        assert!(pages.iter().any(|p| p.slug == "manual") && pages.iter().any(|p| p.slug == "cli"));
        for page in &pages {
            let (html, _) = render_markdown(page.path, page.source, &pages);
            assert!(!html.contains("<script"), "{}", page.slug);
        }
        let manual = pages.iter().find(|p| p.slug == "manual").unwrap();
        let (html, headings) = render_markdown(manual.path, manual.source, &pages);
        assert!(
            html.contains(r#"data-nav href="docs?page=user-guide""#),
            "manual links to the user guide"
        );
        assert!(!headings.is_empty());
    }
}
