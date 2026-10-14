//! Safe text presentation. User Markdown never supplies executable HTML or URLs.
use super::render::escape;
use pulldown_cmark::{html, Event, Options, Parser, Tag, TagEnd};

pub(super) fn highlight(source: &str) -> String {
    let mut result = String::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            let mut word = String::from(c);
            let mut escaped = false;
            for next in chars.by_ref() {
                word.push(next);
                if next == c && !escaped {
                    break;
                }
                escaped = next == '\\' && !escaped;
            }
            result.push_str(&format!(
                "<span class=\"token-string\">{}</span>",
                escape(&word)
            ));
        } else if c.is_ascii_alphanumeric() || c == '_' {
            let mut word = String::from(c);
            while chars
                .peek()
                .is_some_and(|v| v.is_ascii_alphanumeric() || *v == '_')
            {
                word.push(chars.next().unwrap());
            }
            let class = if word.chars().all(|c| c.is_ascii_digit()) {
                "token-number"
            } else if [
                "fn", "pub", "let", "const", "function", "return", "if", "else", "true", "false",
                "None", "Some", "use", "import", "from", "struct", "impl", "def", "class", "async",
                "await", "SELECT", "FROM", "WHERE",
            ]
            .contains(&word.as_str())
            {
                "token-keyword"
            } else {
                ""
            };
            if class.is_empty() {
                result.push_str(&escape(&word));
            } else {
                result.push_str(&format!("<span class=\"{class}\">{}</span>", escape(&word)));
            }
        } else {
            result.push_str(&escape(&c.to_string()));
        }
    }
    result
}

pub(super) fn markdown(source: &str) -> String {
    let mut events = Vec::new();
    let mut code: Option<String> = None;
    for event in Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    ) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(code) = code.take() {
                    events.push(Event::Html(
                        format!("<pre><code>{}</code></pre>", highlight(&code)).into(),
                    ));
                }
            }
            Event::Text(text) if code.is_some() => code.as_mut().unwrap().push_str(&text),
            Event::Html(text) | Event::InlineHtml(text) => events.push(Event::Text(text)),
            Event::Start(Tag::Link { .. } | Tag::Image { .. })
            | Event::End(TagEnd::Link | TagEnd::Image) => {}
            Event::Start(Tag::Heading {
                level,
                id: _,
                classes: _,
                attrs: _,
            }) => {
                let level = match level {
                    pulldown_cmark::HeadingLevel::H1 => 2,
                    pulldown_cmark::HeadingLevel::H2 => 3,
                    _ => 4,
                };
                events.push(Event::Html(format!("<h{level}>").into()));
            }
            Event::End(TagEnd::Heading(level)) => {
                let level = match level {
                    pulldown_cmark::HeadingLevel::H1 => 2,
                    pulldown_cmark::HeadingLevel::H2 => 3,
                    _ => 4,
                };
                events.push(Event::Html(format!("</h{level}>").into()));
            }
            other => events.push(other),
        }
    }
    let mut output = String::new();
    html::push_html(&mut output, events.into_iter());
    output
}

pub(super) fn diff(before: &str, after: &str) -> String {
    let old: Vec<_> = before.lines().collect();
    let new: Vec<_> = after.lines().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut output = String::from("<div class=\"actions\"><button type=\"button\" class=\"quiet\" data-diff-mode=\"unified\" aria-pressed=\"true\">Unified</button><button type=\"button\" class=\"quiet\" data-diff-mode=\"split\" aria-pressed=\"false\">Split</button></div><div class=\"diff\" data-mode=\"unified\">");
    fn unchanged(output: &mut String, lines: &[&str]) {
        if lines.is_empty() {
            return;
        }
        output.push_str(&format!(
            "<details><summary>{} unchanged lines</summary>",
            lines.len()
        ));
        for line in lines {
            output.push_str(&format!(
                "<span class=\"diff-line\"> {}</span>",
                highlight(line)
            ));
        }
        output.push_str("</details>");
    }
    unchanged(&mut output, &old[..prefix]);
    let removed = &old[prefix..old.len() - suffix];
    let added = &new[prefix..new.len() - suffix];
    for index in 0..removed.len().max(added.len()) {
        output.push_str("<div class=\"diff-pair\">");
        if let Some(line) = removed.get(index) {
            output.push_str(&format!(
                "<span class=\"diff-line diff-remove\">- {}</span>",
                highlight(line)
            ));
        } else {
            output.push_str("<span></span>");
        }
        if let Some(line) = added.get(index) {
            output.push_str(&format!(
                "<span class=\"diff-line diff-add\">+ {}</span>",
                highlight(line)
            ));
        } else {
            output.push_str("<span></span>");
        }
        output.push_str("</div>");
    }
    unchanged(&mut output, &old[old.len() - suffix..]);
    output.push_str("</div>");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_and_diff_never_execute_user_markup_or_load_remote_images() {
        let rendered=markdown("# Plan\n<script>alert(1)</script>\n[x](javascript:alert(1))\n![remote](https://evil.invalid/a.png)\n\n```rust\nlet x = \"<img onerror=x>\";\n```");
        assert!(!rendered.contains("<script>"));
        assert!(!rendered.contains("href="));
        assert!(!rendered.contains("<img"));
        assert!(rendered.contains("<h2>Plan</h2>"));
        assert!(rendered.contains("token-keyword"));
        let output = diff("same\n<script>\ntail", "same\nsafe\ntail");
        assert!(output.contains("1 unchanged lines"));
        assert!(output.contains("&lt;script&gt;"));
        assert!(output.contains(r#"data-diff-mode="unified" aria-pressed="true">Unified</button><button type="button" class="quiet" data-diff-mode="split" aria-pressed="false">Split</button></div><div class="diff" data-mode="unified">"#));
    }
}
