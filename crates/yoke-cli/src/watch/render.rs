//! Pure rendering of a snapshot into terminal lines of a given width and height.

use yoke_core::quota::{self, Window};
use yoke_core::timefmt;

use super::{Counts, Filter, Snapshot, StateNote, Trust};

/// Most events a snapshot without a height lists.
const SNAPSHOT_EVENTS: usize = 20;
/// Event rows the live view keeps on screen before it drops a lower-priority block.
const MIN_EVENT_ROWS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Plain,
    Dim,
    Bold,
    Good,
    Warn,
    Bad,
    Error,
}

impl Tone {
    fn sgr(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Dim => Some("2"),
            Self::Bold => Some("1"),
            Self::Good => Some("32"),
            Self::Warn => Some("33"),
            Self::Bad => Some("31"),
            Self::Error => Some("35"),
        }
    }
}

/// How the frame is drawn: its size and which escape codes and characters the terminal gets.
pub(crate) struct Frame {
    pub width: usize,
    /// `None` prints every block in full, for a one-off snapshot.
    pub height: Option<usize>,
    pub color: bool,
    pub ascii: bool,
}

struct Glyphs {
    sep: &'static str,
    more: &'static str,
    used: char,
    free: char,
}

impl Frame {
    fn glyphs(&self) -> Glyphs {
        if self.ascii {
            Glyphs {
                sep: " | ",
                more: "...",
                used: '#',
                free: '-',
            }
        } else {
            Glyphs {
                sep: " \u{b7} ",
                more: "\u{2026}",
                used: '\u{2501}',
                free: '\u{2500}',
            }
        }
    }
}

/// One screen line as styled spans; only the visible text counts toward the width.
#[derive(Default)]
struct Line(Vec<(Tone, String)>);

impl Line {
    fn new() -> Self {
        Self::default()
    }

    fn push(mut self, tone: Tone, text: impl Into<String>) -> Self {
        self.0.push((tone, text.into()));
        self
    }

    fn text(self, text: impl Into<String>) -> Self {
        self.push(Tone::Plain, text)
    }

    fn width(&self) -> usize {
        self.0.iter().map(|(_, text)| text.chars().count()).sum()
    }

    /// Cut to `width` visible characters, ending in the ellipsis when text was cut.
    fn draw(&self, width: usize, frame: &Frame, glyphs: &Glyphs) -> String {
        let cut = self.width() > width;
        let mut room = if cut {
            width.saturating_sub(glyphs.more.chars().count())
        } else {
            width
        };
        let mut out = String::new();
        for (tone, text) in &self.0 {
            if room == 0 {
                break;
            }
            let piece: String = text.chars().take(room).collect();
            room -= piece.chars().count();
            match tone.sgr().filter(|_| frame.color) {
                Some(code) => out.push_str(&format!("\x1b[{code}m{piece}\x1b[0m")),
                None => out.push_str(&piece),
            }
        }
        if cut && width > 0 {
            out.push_str(if width >= glyphs.more.chars().count() {
                glyphs.more
            } else {
                ""
            });
        }
        out
    }
}

/// Printable text only: stored values can hold newlines or escape codes that would break the
/// layout or reach the terminal.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn pad(text: &str, width: usize) -> String {
    let text: String = clean(text).chars().take(width).collect();
    format!("{text:<width$}")
}

fn clock(epoch: i64, offset: i64) -> String {
    timefmt::full_time_secs(epoch, offset)
        .split_once(' ')
        .map_or_else(String::new, |(_, time)| time.to_owned())
}

/// `7s`, else the coarser `3h 12m` form.
fn age(secs: i64) -> String {
    if (0..60).contains(&secs) {
        format!("{secs}s")
    } else {
        timefmt::duration_text(secs)
    }
}

/// `21:34:05 (7s ago)` today, the short date otherwise.
fn when(epoch: i64, now: i64, offset: i64) -> String {
    if timefmt::day_start(epoch, offset) == timefmt::day_start(now, offset) {
        format!(
            "{} ({} ago)",
            clock(epoch, offset),
            age(now.saturating_sub(epoch))
        )
    } else {
        timefmt::short_time(epoch, now, offset)
    }
}

fn count(value: u64, partial: bool) -> String {
    if partial {
        format!("{value}+")
    } else {
        value.to_string()
    }
}

fn outcome_tone(outcome: &str) -> Tone {
    match outcome {
        "deny" => Tone::Bad,
        "would_deny" => Tone::Warn,
        "error" => Tone::Error,
        "allow" | "rewrite" => Tone::Good,
        _ => Tone::Plain,
    }
}

fn quota_tone(used: f64) -> Tone {
    if used >= 80.0 {
        Tone::Bad
    } else if used >= 50.0 {
        Tone::Warn
    } else {
        Tone::Good
    }
}

fn header(s: &Snapshot, frame: &Frame, live: bool, glyphs: &Glyphs) -> Vec<Line> {
    // The version goes first and then the clock when the line is too wide for the terminal.
    let identity = |version: bool, time: bool| {
        let mut line = Line::new().push(Tone::Bold, "yoke watch").text(glyphs.sep);
        line = line.push(Tone::Bold, clean(&s.project_name));
        if let Some(branch) = &s.branch {
            line = line.text(glyphs.sep).text(clean(branch));
            match s.changed {
                Some(0) => line = line.push(Tone::Dim, " clean"),
                Some(n) => line = line.push(Tone::Warn, format!(" {n} changed")),
                None => {}
            }
        }
        if version {
            line = line
                .text(glyphs.sep)
                .push(Tone::Dim, format!("v{}", s.version));
        }
        if time {
            line = line.text(glyphs.sep).text(clock(s.now, s.offset));
        }
        line
    };
    let first = [(true, true), (false, true)]
        .into_iter()
        .map(|(version, time)| identity(version, time))
        .find(|line| line.width() <= frame.width)
        .unwrap_or_else(|| identity(false, false));
    let mut second = Line::new().text("mode ");
    second = match &s.mode {
        Some(mode) => second.push(Tone::Bold, clean(mode)),
        None => second.push(Tone::Dim, "unknown"),
    };
    second = second.text(glyphs.sep);
    second = match &s.trust {
        Trust::Trusted {
            gates,
            stale: false,
        } => second.push(Tone::Good, format!("trusted policy, {gates} gates")),
        Trust::Trusted { gates, stale: true } => second.push(
            Tone::Warn,
            format!("trusted policy, {gates} gates; a policy file changed since it was trusted"),
        ),
        Trust::Untrusted => second.push(Tone::Warn, "policy not trusted: gates are not running"),
        Trust::NoPolicy => second.push(Tone::Warn, "no policy: gates are not running"),
        Trust::Unknown(error) => {
            second.push(Tone::Bad, format!("policy unreadable: {}", clean(error)))
        }
    };
    let mut lines = vec![first, second];
    // An older schema is not a problem to show here: blocks that need the newer one say so.
    let (tone, note) = match &s.state {
        StateNote::Ready | StateNote::Old => return lines,
        StateNote::Missing => (
            Tone::Dim,
            "No Yoke state yet; the first hook event creates it. Waiting for it.".into(),
        ),
        StateNote::Problem(problem) => (Tone::Bad, clean(problem)),
    };
    // The note says how to recover, so the live view gives it two lines and a snapshot all.
    let max = if live { 2 } else { usize::MAX };
    for chunk in wrap(&note, frame.width, max) {
        lines.push(Line::new().push(tone, chunk));
    }
    lines
}

/// Whether the state answered at least once, so missing rows mean nothing was recorded.
fn readable(s: &Snapshot) -> bool {
    matches!(s.state, StateNote::Ready | StateNote::Old)
}

fn agents(s: &Snapshot, frame: &Frame) -> Vec<Line> {
    // The last-event column is left out rather than cut into the counts before it.
    let room = |line: &Line, tail: &Line| line.width() + 2 + tail.width() <= frame.width;
    let columns = |label: &str, c: &Counts, partial: bool, tail: Line| {
        let mut line = Line::new().text(pad(label, 16));
        for (value, width, tone) in [
            (c.allowed, 8, Tone::Plain),
            (c.would_deny, 11, Tone::Warn),
            (c.denied, 7, Tone::Bad),
            (c.errors, 7, Tone::Error),
        ] {
            let text = format!("{:>width$}", count(value, partial));
            line = line.push(if value > 0 { tone } else { Tone::Plain }, text);
        }
        if room(&line, &tail) {
            line = line.text("  ");
            line.0.extend(tail.0);
        }
        line
    };
    let heading = Line::new().push(Tone::Bold, pad("Agents", 16)).push(
        Tone::Dim,
        format!(
            "{:>8}{:>11}{:>7}{:>7}",
            "allowed", "would-deny", "denied", "errors"
        ),
    );
    let last = Line::new().text("last event");
    let mut lines = vec![if room(&heading, &last) {
        heading.push(Tone::Dim, "  last event")
    } else {
        heading
    }];
    if s.agents.is_empty() {
        lines.push(Line::new().push(
            Tone::Dim,
            match s.state {
                StateNote::Missing => "No decisions yet.",
                _ => "Counts appear once the state can be read.",
            },
        ));
    }
    for agent in &s.agents {
        let last = agent.last_event.map_or_else(
            || Line::new().push(Tone::Dim, "none yet"),
            |ts| Line::new().text(when(ts, s.now, s.offset)),
        );
        lines.push(columns(
            &format!("{:<8}today", agent.name),
            &agent.today,
            agent.today_partial,
            last,
        ));
        if let Some(session) = &agent.session {
            let id: String = clean(&session.id).chars().take(8).collect();
            let mut tail = if session.open {
                format!("session {id}")
            } else {
                format!("ended session {id}")
            };
            if let Some(since) = session.since {
                tail.push_str(&format!(
                    ", counted since {}",
                    timefmt::short_time(since, s.now, s.offset)
                ));
            }
            let tail = Line::new().push(Tone::Dim, tail);
            lines.push(columns(
                "        session",
                &session.counts,
                session.partial,
                tail,
            ));
        }
    }
    lines
}

fn bar(window: &Window, width: usize, glyphs: &Glyphs) -> String {
    let used = ((window.used_percent / 100.0) * width as f64).round() as usize;
    let used = used.min(width);
    let mut text: String = std::iter::repeat_n(glyphs.used, used).collect();
    text.extend(std::iter::repeat_n(glyphs.free, width - used));
    text
}

/// `5h  ━━━━────── 38%  resets 23:10` for one window.
fn window(window: &Window, s: &Snapshot, bar_width: usize, glyphs: &Glyphs) -> Line {
    let tone = quota_tone(window.used_percent);
    let line = Line::new()
        .text(format!("{:<4}", window.label()))
        .push(tone, bar(window, bar_width, glyphs))
        .push(
            tone,
            format!(" {:>4}%", quota::percent(window.used_percent)),
        );
    if window.expired(s.now) {
        line.push(Tone::Dim, "  reset passed; last reading")
    } else {
        line.text(format!(
            "  resets {}",
            timefmt::short_time(window.resets_at, s.now, s.offset)
        ))
    }
}

/// One line per agent when its windows fit side by side, else one line per window.
fn quota(s: &Snapshot, frame: &Frame, glyphs: &Glyphs) -> Vec<Line> {
    let mut lines = vec![Line::new().push(Tone::Bold, "Quota")];
    for row in &s.quota {
        let label = Line::new().text(format!("{:<8}", row.agent));
        if let Some(note) = &row.note {
            lines.push(label.push(Tone::Dim, clean(note)));
            continue;
        }
        let side_by_side = row
            .windows
            .iter()
            .enumerate()
            .fold(label, |mut line, (i, w)| {
                if i > 0 {
                    line = line.text("    ");
                }
                line.0.extend(window(w, s, 10, glyphs).0);
                line
            });
        if side_by_side.width() <= frame.width {
            lines.push(side_by_side);
            continue;
        }
        let bar_width = (frame.width / 5).clamp(5, 20);
        for (i, w) in row.windows.iter().enumerate() {
            let mut line = Line::new().text(if i == 0 {
                format!("{:<8}", row.agent)
            } else {
                " ".repeat(8)
            });
            line.0.extend(window(w, s, bar_width, glyphs).0);
            lines.push(line);
        }
    }
    lines
}

fn status(s: &Snapshot, width: usize, glyphs: &Glyphs) -> Vec<Line> {
    let label = |name: &str| Line::new().push(Tone::Bold, pad(name, 10));
    let mut lines = Vec::new();
    lines.push(match &s.plans {
        None => label("Plans").push(Tone::Dim, "waiting for state"),
        Some(Err(error)) => label("Plans").push(Tone::Dim, clean(error)),
        Some(Ok(plans)) if plans.is_empty() => label("Plans").push(Tone::Dim, "none active"),
        Some(Ok(plans)) => {
            let mut line = label("Plans");
            for (index, (slug, state)) in plans.iter().enumerate() {
                if index > 0 {
                    line = line.text(glyphs.sep);
                }
                let tone = match state.as_str() {
                    "stalled" | "failed" => Tone::Bad,
                    "waiting" | "verifying" => Tone::Warn,
                    _ => Tone::Plain,
                };
                line = line
                    .text(format!("{} ", clean(slug)))
                    .push(tone, clean(state));
            }
            line
        }
    });
    match &s.attention {
        None => lines.push(label("Attention").push(Tone::Dim, "waiting for state")),
        Some(Err(error)) => lines.push(label("Attention").push(Tone::Dim, clean(error))),
        Some(Ok(attention)) if attention.unread == 0 => {
            lines.push(label("Attention").push(Tone::Dim, "nothing unread"))
        }
        Some(Ok(attention)) => {
            let mut first = true;
            for (title, next) in &attention.items {
                let lead = if first {
                    label("Attention").push(Tone::Warn, format!("{} unread: ", attention.unread))
                } else {
                    Line::new().text(" ".repeat(10))
                };
                first = false;
                lines.push(
                    lead.text(clean(title))
                        .push(Tone::Dim, format!(" ({})", clean(next))),
                );
            }
            let more = attention.unread - attention.items.len() as i64;
            if more > 0 {
                lines.push(
                    Line::new()
                        .text(" ".repeat(10))
                        .push(Tone::Dim, format!("{more} more: yoke notifications")),
                );
            }
        }
    }
    lines.push(match &s.jobs {
        None => label("Jobs").push(Tone::Dim, "waiting for state"),
        Some(Err(error)) => label("Jobs").push(Tone::Dim, clean(error)),
        Some(Ok(jobs)) if jobs.is_empty() => label("Jobs").push(Tone::Dim, "none pending"),
        Some(Ok(jobs)) => {
            let mut line = label("Jobs");
            for (index, (state, n)) in jobs.iter().enumerate() {
                if index > 0 {
                    line = line.text(glyphs.sep);
                }
                let tone = if state == "dead" {
                    Tone::Bad
                } else {
                    Tone::Plain
                };
                line = line.push(tone, format!("{n} {state}"));
            }
            line.push(Tone::Dim, "  (yoke jobs)")
        }
    });
    let parts: Vec<Line> = s
        .hooks
        .iter()
        .map(|hook| {
            let part = Line::new().text(format!("{} ", hook.agent));
            let part = match hook.installed {
                Some(true) => part.push(Tone::Good, "installed"),
                Some(false) => part.push(Tone::Warn, "not installed"),
                None => part.push(Tone::Dim, "checking"),
            };
            match hook.last_event {
                Some(ts) => part.text(format!(
                    ", last event {} ago",
                    age(s.now.saturating_sub(ts))
                )),
                None if readable(s) || s.state == StateNote::Missing => {
                    part.push(Tone::Dim, ", no event yet")
                }
                None => part,
            }
        })
        .collect();
    // One line when it fits, else one line per agent so neither is cut off.
    let mut joined = label("Hooks");
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            joined = joined.text(glyphs.sep);
        }
        joined.0.extend(part.0.iter().cloned());
    }
    if joined.width() <= width {
        lines.push(joined);
    } else {
        for (index, part) in parts.into_iter().enumerate() {
            let mut line = if index == 0 {
                label("Hooks")
            } else {
                Line::new().text(" ".repeat(10))
            };
            line.0.extend(part.0);
            lines.push(line);
        }
    }
    lines
}

/// Columns of the event list that fit `width`; the detail takes what is left.
fn event_columns(width: usize) -> (bool, bool, bool) {
    // (event, tool, gate)
    (width >= 100, width >= 56, width >= 72)
}

fn events(s: &Snapshot, frame: &Frame, glyphs: &Glyphs, rows: Option<usize>) -> Vec<Line> {
    let title = match s.filter {
        Filter::All => "all",
        Filter::Denied => "denied and would-deny only",
        Filter::Errors => "errors only",
    };
    let mut heading = Line::new().push(Tone::Bold, "Events").push(
        Tone::Dim,
        format!("{}{title}{}newest first", glyphs.sep, glyphs.sep),
    );
    if let Some((median, max)) = s.gate_ms {
        heading = heading.push(
            Tone::Dim,
            format!("{}gate time {median} ms median, {max} ms max", glyphs.sep),
        );
    }
    let mut lines = vec![heading];
    if s.events.is_empty() {
        lines.push(Line::new().push(
            Tone::Dim,
            match (s.filter, &s.state) {
                (_, StateNote::Missing) => "No decisions yet.",
                (_, StateNote::Problem(_)) => "Events appear once the state can be read.",
                (Filter::All, _) => "No decisions recorded for this project yet.",
                (Filter::Denied, _) => "No denied or would-deny decisions in this project.",
                (Filter::Errors, _) => "No errors in this project.",
            },
        ));
        return lines;
    }
    let (show_event, show_tool, show_gate) = event_columns(frame.width);
    let limit = rows.unwrap_or(SNAPSHOT_EVENTS);
    for event in s.events.iter().take(limit) {
        let mut line = Line::new()
            .text(format!("{}  ", clock(event.ts, s.offset)))
            .text(format!("{}  ", pad(&event.agent, 6)));
        if show_event {
            line = line.push(Tone::Dim, format!("{}  ", pad(&event.event, 18)));
        }
        if show_tool {
            line = line.text(format!(
                "{}  ",
                pad(event.tool.as_deref().unwrap_or("-"), 10)
            ));
        }
        let outcome = event.outcome.replace('_', "-");
        line = line.push(
            outcome_tone(&event.outcome),
            format!("{}  ", pad(&outcome, 10)),
        );
        if show_gate {
            line = line.push(
                if event.gate.is_empty() {
                    Tone::Dim
                } else {
                    Tone::Plain
                },
                format!(
                    "{}  ",
                    pad(
                        if event.gate.is_empty() {
                            "-"
                        } else {
                            &event.gate
                        },
                        16
                    )
                ),
            );
        }
        lines.push(line.text(clean(&event.detail)));
    }
    lines
}

/// Words of `text` filled into lines of `width`; past `max` lines the rest joins the last one,
/// which drawing then cuts.
fn wrap(text: &str, width: usize, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let full = lines.len() >= max;
        match lines.last_mut() {
            Some(line) if full || line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

fn footer(s: &Snapshot, frame: &Frame, live: bool, glyphs: &Glyphs) -> Vec<Line> {
    const LEAD: &str = "next  ";
    let (tone, text) = s
        .next
        .clone()
        .unwrap_or((Tone::Dim, "checking yoke health".into()));
    // The live view gives the step two lines; a snapshot prints all of it.
    let max = if live { 2 } else { usize::MAX };
    let room = frame.width.saturating_sub(LEAD.len()).max(10);
    let mut lines: Vec<Line> = wrap(&clean(&text), room, max)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let lead = if index == 0 {
                Line::new().push(Tone::Bold, LEAD)
            } else {
                Line::new().text(" ".repeat(LEAD.len()))
            };
            lead.push(tone, chunk)
        })
        .collect();
    let keys = if live {
        let filter = match s.filter {
            Filter::All => "all",
            Filter::Denied => "denied",
            Filter::Errors => "errors",
        };
        Line::new().push(
            Tone::Dim,
            format!(
                "q quit{sep}f filter: {filter}{sep}r refresh{sep}read-only",
                sep = glyphs.sep
            ),
        )
    } else {
        Line::new().push(
            Tone::Dim,
            "One snapshot; run yoke watch in a terminal for the live view.",
        )
    };
    lines.push(keys);
    lines
}

/// The snapshot as lines of at most `frame.width` visible characters and, when the frame has a
/// height, exactly that many lines: lower-priority blocks go first so events keep some room.
pub(crate) fn render(s: &Snapshot, frame: &Frame, live: bool) -> Vec<String> {
    let glyphs = frame.glyphs();
    let header = header(s, frame, live, &glyphs);
    let footer = footer(s, frame, live, &glyphs);
    let mut blocks = vec![
        agents(s, frame),
        quota(s, frame, &glyphs),
        status(s, frame.width, &glyphs),
    ];
    let lines: Vec<Line> = match frame.height {
        None => {
            let mut lines = header;
            lines.extend(blocks.into_iter().flatten());
            lines.extend(events(s, frame, &glyphs, None));
            lines.extend(footer);
            lines
        }
        Some(height) => {
            let fixed = |blocks: &Vec<Vec<Line>>| {
                header.len() + footer.len() + blocks.iter().map(Vec::len).sum::<usize>()
            };
            // Quota, then plans and the rest, then the agent table give way to events.
            for drop in [1, 1, 0] {
                if height.saturating_sub(fixed(&blocks)) > MIN_EVENT_ROWS || blocks.len() <= drop {
                    break;
                }
                blocks.remove(drop);
            }
            let rows = height.saturating_sub(fixed(&blocks) + 1);
            let mut lines = header;
            lines.extend(blocks.into_iter().flatten());
            lines.extend(events(s, frame, &glyphs, Some(rows)));
            let room = height.saturating_sub(footer.len());
            lines.truncate(room);
            while lines.len() < room {
                lines.push(Line::new());
            }
            lines.extend(footer);
            lines.truncate(height);
            lines
        }
    };
    lines
        .iter()
        .map(|line| line.draw(frame.width, frame, &glyphs))
        .collect()
}
