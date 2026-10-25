//! Wording, layout and color of the statusline. It is pure: facts in, lines out, so every
//! segment, width and display mode is testable without a terminal, a repository or a database.
//!
//! The line is a row of segments separated by ` | `. When it is wider than the terminal, whole
//! segments are dropped, least important first, and it never wraps. Color marks a level, never
//! carries meaning alone: every colored word is also spelled out.

use crate::terminal::{quota_tone as level, Tone};
use yoke_core::quota::{self, Window};
use yoke_core::redact::redact;
use yoke_core::store::SessionCounts;
use yoke_core::timefmt;

const BAR_CELLS: usize = 8;
const SEPARATOR: &str = " | ";
/// Written as escapes so the source stays 7-bit; `YOKE_ASCII=1` replaces all of them.
const ELLIPSIS: &str = "\u{2026}";
const MIDDLE_DOT: &str = " \u{b7} ";
const FULL: &str = "\u{2588}";
const SHADE: &str = "\u{2591}";

#[derive(Clone, Debug)]
pub(super) struct Look {
    pub color: bool,
    /// Plain 7-bit characters only, for terminals and fonts that lack the rest.
    pub ascii: bool,
    /// Add the second, detail line.
    pub detailed: bool,
    /// Columns the first line may use, or 0 for no limit.
    pub width: usize,
}

/// The git state of the session's directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Branch {
    pub name: String,
    /// `name` is a short commit id because HEAD points at no branch.
    pub detached: bool,
    pub dirty: Option<Dirty>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Dirty {
    pub files: u32,
    /// More files changed than were counted.
    pub capped: bool,
}

/// Everything the line can show. Whatever is absent is left out, never guessed.
#[derive(Default)]
pub(super) struct Facts {
    pub mode: Option<String>,
    /// `Some(false)` when state exists but no trusted configuration applies.
    pub trusted: Option<bool>,
    pub branch: Option<Branch>,
    pub counts: Option<SessionCounts>,
    pub plan: Option<(String, String)>,
    pub recent_deny: bool,
    pub quota: Vec<Window>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub lines: Option<(u64, u64)>,
    pub context: Option<f64>,
    pub model: Option<String>,
    pub session: Option<String>,
    pub unread: i64,
    pub problem: Option<String>,
    pub update: Option<String>,
}

/// Importance when the line is too wide: the highest rank is dropped first, rank 0 never.
mod rank {
    pub const MODE: u8 = 0;
    pub const ALERTS: u8 = 1;
    /// A failing health check means Yoke may protect nothing, which outranks everything else.
    pub const PROBLEM: u8 = 2;
    pub const GIT: u8 = 3;
    pub const DECISIONS: u8 = 4;
    pub const PLAN: u8 = 5;
    pub const QUOTA: u8 = 6;
    pub const CONTEXT: u8 = 7;
    pub const COST: u8 = 8;
    pub const UPDATE: u8 = 9;
}

struct Segment {
    rank: u8,
    parts: Vec<(Tone, String)>,
}

impl Segment {
    fn new(rank: u8, parts: Vec<(Tone, String)>) -> Option<Self> {
        (!parts.is_empty()).then_some(Self { rank, parts })
    }

    fn one(rank: u8, tone: Tone, text: String) -> Option<Self> {
        Self::new(rank, vec![(tone, text)])
    }

    fn width(&self) -> usize {
        self.parts
            .iter()
            .map(|(_, text)| text.chars().count())
            .sum()
    }

    fn paint(&self, color: bool) -> String {
        self.parts
            .iter()
            .map(|(tone, text)| tone.paint(text, color))
            .collect()
    }

    fn plain(&self) -> String {
        self.parts.iter().map(|(_, text)| text.as_str()).collect()
    }
}

/// Text from outside Yoke made safe to paint: secrets redacted, control characters (which
/// include escape sequences) removed, 7-bit only in ASCII mode, and cut to `limit` characters.
fn text(raw: &str, look: &Look, limit: usize) -> String {
    let cleaned: Vec<char> = redact(raw)
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if look.ascii && !c.is_ascii() { '?' } else { c })
        .collect();
    if cleaned.len() <= limit {
        return cleaned.into_iter().collect();
    }
    let (mark, keep) = if look.ascii {
        ("...", limit.saturating_sub(3))
    } else {
        (ELLIPSIS, limit.saturating_sub(1))
    };
    cleaned.into_iter().take(keep).collect::<String>() + mark
}

fn joiner(look: &Look) -> &'static str {
    if look.ascii {
        ", "
    } else {
        MIDDLE_DOT
    }
}

fn mode(facts: &Facts, look: &Look) -> Option<Segment> {
    let (tone, name) = match facts.mode.as_deref() {
        Some("enforce") => (Tone::Good, "yoke enforce".to_owned()),
        Some(mode) => (Tone::Plain, format!("yoke {}", text(mode, look, 24))),
        None => (Tone::Dim, "yoke (no data yet)".to_owned()),
    };
    let mut parts = vec![(tone, name)];
    if facts.trusted == Some(false) {
        parts.push((Tone::Warn, " (no trusted policy)".to_owned()));
    }
    Segment::new(rank::MODE, parts)
}

fn git(facts: &Facts, look: &Look) -> Option<Segment> {
    let branch = facts.branch.as_ref()?;
    let name = text(&branch.name, look, 32);
    let mut parts = vec![(
        Tone::Plain,
        if branch.detached {
            format!("detached {name}")
        } else {
            name
        },
    )];
    if let Some(dirty) = branch.dirty.filter(|dirty| dirty.files > 0) {
        let more = if dirty.capped { "+" } else { "" };
        parts.push((Tone::Warn, format!(" ({}{more} changed)", dirty.files)));
    }
    Segment::new(rank::GIT, parts)
}

fn decisions(facts: &Facts, look: &Look) -> Option<Segment> {
    let counts = facts.counts.as_ref()?;
    let enforcing = facts.mode.as_deref() == Some("enforce");
    let tone = |n: i64, active: Tone| if n > 0 { active } else { Tone::Plain };
    let mut items = vec![(Tone::Plain, format!("allowed {}", counts.allowed))];
    // The kind of refusal this mode produces is always shown, so a zero is a fact, not a gap.
    if !enforcing || counts.would_deny > 0 {
        items.push((
            tone(counts.would_deny, Tone::Warn),
            format!("would-deny {}", counts.would_deny),
        ));
    }
    if enforcing || counts.blocked > 0 {
        items.push((
            tone(counts.blocked, Tone::Bad),
            format!("blocked {}", counts.blocked),
        ));
    }
    if counts.errors > 0 {
        items.push((Tone::Bad, format!("errors {}", counts.errors)));
    }
    let mut parts = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            parts.push((Tone::Plain, joiner(look).to_owned()));
        }
        parts.push(item);
    }
    Segment::new(rank::DECISIONS, parts)
}

fn plan(facts: &Facts, look: &Look) -> Option<Segment> {
    let (slug, state) = facts.plan.as_ref()?;
    let tone = match state.as_str() {
        "stalled" => Tone::Bad,
        "waiting" => Tone::Warn,
        _ => Tone::Plain,
    };
    let mut parts = vec![
        (Tone::Plain, format!("plan {} ", text(slug, look, 40))),
        (tone, text(state, look, 16)),
    ];
    // In enforce mode a Stop is held while the plan is being built or checked.
    if facts.mode.as_deref() == Some("enforce") && matches!(state.as_str(), "building" | "checking")
    {
        parts.push((Tone::Plain, joiner(look).to_owned()));
        parts.push((Tone::Warn, "Stop held".to_owned()));
    }
    Segment::new(rank::PLAN, parts)
}

/// The window closest to its limit, which is the one that will block work first.
fn binding(windows: &[&Window]) -> Option<Window> {
    windows
        .iter()
        .fold(None::<&&Window>, |best, w| match best {
            Some(b) if b.used_percent >= w.used_percent => Some(b),
            _ => Some(w),
        })
        .map(|w| (*w).clone())
}

fn live(facts: &Facts, now: i64) -> Vec<&Window> {
    facts.quota.iter().filter(|w| !w.expired(now)).collect()
}

fn quota_segment(facts: &Facts, look: &Look, now: i64, offset: i64) -> Option<Segment> {
    let windows = live(facts, now);
    let nearest = binding(&windows)?;
    let mut parts = Vec::new();
    for (i, w) in windows.iter().enumerate() {
        if i > 0 {
            parts.push((Tone::Plain, joiner(look).to_owned()));
        }
        parts.push((
            level(w.used_percent),
            format!("{} {}%", w.label(), quota::percent(w.used_percent)),
        ));
    }
    parts.push((
        Tone::Dim,
        format!(
            " (resets {})",
            timefmt::short_time(nearest.resets_at, now, offset)
        ),
    ));
    Segment::new(rank::QUOTA, parts)
}

fn context(facts: &Facts, look: &Look) -> Option<Segment> {
    let percent = facts.context?;
    let tone = level(percent);
    let filled = ((percent / 100.0 * BAR_CELLS as f64).round() as usize).min(BAR_CELLS);
    let bar = if look.ascii {
        format!("[{}{}]", "#".repeat(filled), "-".repeat(BAR_CELLS - filled))
    } else {
        format!(
            "{}{}",
            FULL.repeat(filled),
            SHADE.repeat(BAR_CELLS - filled)
        )
    };
    Segment::new(
        rank::CONTEXT,
        vec![
            (Tone::Plain, "ctx ".to_owned()),
            (tone, format!("{percent:.0}% {bar}")),
        ],
    )
}

fn money(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        "<$0.01".into()
    } else {
        format!("${usd:.2}")
    }
}

fn cost(facts: &Facts, look: &Look) -> Option<Segment> {
    let mut items = Vec::new();
    if let Some(usd) = facts.cost_usd.filter(|usd| usd.is_finite() && *usd >= 0.0) {
        items.push(money(usd));
    }
    if let Some(ms) = facts.duration_ms {
        items.push(timefmt::duration_text(
            i64::try_from(ms / 1000).unwrap_or(i64::MAX),
        ));
    }
    if let Some((added, removed)) = facts.lines.filter(|(a, r)| a + r > 0) {
        items.push(format!("+{added}/-{removed}"));
    }
    Segment::one(rank::COST, Tone::Plain, items.join(joiner(look))).filter(|_| !items.is_empty())
}

fn alerts(facts: &Facts, look: &Look) -> Option<Segment> {
    let stalled = facts
        .plan
        .as_ref()
        .is_some_and(|(_, state)| state == "stalled");
    let mut items = Vec::new();
    if stalled {
        items.push("stalled".to_owned());
    } else if facts.recent_deny {
        items.push("recent deny".to_owned());
    }
    if facts.unread > 0 {
        items.push(format!("{} unread", facts.unread));
    }
    if items.is_empty() {
        return None;
    }
    let tone = if stalled { Tone::Bad } else { Tone::Warn };
    Segment::one(
        rank::ALERTS,
        tone,
        format!("! {}", items.join(joiner(look))),
    )
}

fn problem(facts: &Facts, look: &Look, limit: usize) -> Option<Segment> {
    let problem = facts.problem.as_deref()?;
    Segment::one(
        rank::PROBLEM,
        Tone::Warn,
        format!("! {}", text(problem, look, limit)),
    )
}

fn update(facts: &Facts, look: &Look) -> Option<Segment> {
    let latest = facts.update.as_deref()?;
    Segment::one(
        rank::UPDATE,
        Tone::Dim,
        format!("update {}", text(latest, look, 24)),
    )
}

/// Drop whole segments, least important first, until the line fits; never wrap or cut a segment
/// other than the mode, which is cut as a last resort.
fn fit(mut segments: Vec<Segment>, width: usize, look: &Look) -> String {
    let total = |segments: &[Segment]| {
        segments.iter().map(Segment::width).sum::<usize>()
            + SEPARATOR.len() * segments.len().saturating_sub(1)
    };
    while width > 0 && total(&segments) > width {
        let droppable = segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| segment.rank > rank::MODE)
            .max_by_key(|(at, segment)| (segment.rank, *at))
            .map(|(at, _)| at);
        match droppable {
            Some(at) => {
                segments.remove(at);
            }
            None => break,
        }
    }
    if width > 0 && total(&segments) > width {
        // Only the mode is left and it alone is too wide.
        let first = segments.first().map(Segment::plain).unwrap_or_default();
        return text(&first, look, width);
    }
    segments
        .iter()
        .map(|segment| segment.paint(look.color))
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

fn ago(seconds: i64) -> String {
    if seconds < 60 {
        format!("{}s ago", seconds.max(0))
    } else {
        format!("{} ago", timefmt::duration_text(seconds))
    }
}

/// The second line: what the first has no room for.
fn details(facts: &Facts, look: &Look, now: i64, offset: i64) -> Option<String> {
    let mut segments = Vec::new();
    if let Some((at, event, tool)) = facts
        .counts
        .as_ref()
        .and_then(|counts| counts.last.as_ref())
    {
        let what = tool
            .as_deref()
            .filter(|tool| !tool.is_empty())
            .unwrap_or(event);
        segments.extend(Segment::one(
            2,
            Tone::Plain,
            format!("last {} {}", text(what, look, 24), ago(now - at)),
        ));
    }
    if let Some(model) = facts.model.as_deref() {
        segments.extend(Segment::one(
            3,
            Tone::Plain,
            format!("model {}", text(model, look, 24)),
        ));
    }
    let windows = live(facts, now);
    if !windows.is_empty() {
        let mut parts = Vec::new();
        for (i, w) in windows.iter().enumerate() {
            if i > 0 {
                parts.push((Tone::Plain, joiner(look).to_owned()));
            }
            parts.push((
                level(w.used_percent),
                format!(
                    "{} {}% resets {}",
                    w.label(),
                    quota::percent(w.used_percent),
                    timefmt::short_time(w.resets_at, now, offset)
                ),
            ));
        }
        segments.extend(Segment::new(4, parts));
    }
    if let Some(session) = facts.session.as_deref() {
        segments.extend(Segment::one(
            5,
            Tone::Dim,
            format!(
                "session {}",
                text(session, look, 64).chars().take(8).collect::<String>()
            ),
        ));
    }
    // The detailed style gives the problem its full text here, so the first line omits it.
    // It is the last thing this line gives up.
    if let Some(problem) = problem(facts, look, 120) {
        segments.push(Segment { rank: 1, ..problem });
    }
    let line = fit(segments, look.width, look);
    (!line.is_empty()).then_some(line)
}

/// One line, or two in the detailed style when there is anything to put on the second.
pub(super) fn render(facts: &Facts, look: &Look, now: i64, offset: i64) -> Vec<String> {
    let segments: Vec<Segment> = [
        mode(facts, look),
        git(facts, look),
        decisions(facts, look),
        plan(facts, look),
        quota_segment(facts, look, now, offset),
        cost(facts, look),
        context(facts, look),
        alerts(facts, look),
        problem(facts, look, 48).filter(|_| !look.detailed),
        update(facts, look),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut lines = vec![fit(segments, look.width, look)];
    if look.detailed {
        lines.extend(details(facts, look, now, offset));
    }
    lines
}

#[cfg(test)]
#[path = "line_tests.rs"]
mod tests;
