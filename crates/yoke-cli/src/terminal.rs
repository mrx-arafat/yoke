//! Shared terminal colors. Layout measures plain text before applying escape sequences.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tone {
    Plain,
    Dim,
    Good,
    Warn,
    Bad,
}

impl Tone {
    pub(crate) fn paint(self, text: &str, color: bool) -> String {
        let code = match self {
            Self::Plain => None,
            Self::Dim => Some("2"),
            Self::Good => Some("32"),
            Self::Warn => Some("33"),
            Self::Bad => Some("31"),
        };
        match (color, code) {
            (true, Some(code)) => format!("\x1b[{code}m{text}\x1b[0m"),
            _ => text.to_owned(),
        }
    }
}

pub(crate) fn quota_tone(used_percent: f64) -> Tone {
    if used_percent >= 85.0 {
        Tone::Bad
    } else if used_percent >= 60.0 {
        Tone::Warn
    } else {
        Tone::Good
    }
}

pub(crate) fn color_allowed(terminal: bool, no_color: Option<&str>, term: Option<&str>) -> bool {
    terminal && no_color.is_none_or(str::is_empty) && term != Some("dumb")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_colors_keep_the_statusline_thresholds_and_can_be_disabled() {
        for (used, tone, code) in [
            (0.0, Tone::Good, "32"),
            (59.9, Tone::Good, "32"),
            (60.0, Tone::Warn, "33"),
            (84.9, Tone::Warn, "33"),
            (85.0, Tone::Bad, "31"),
            (100.0, Tone::Bad, "31"),
        ] {
            assert_eq!(quota_tone(used), tone);
            assert_eq!(
                tone.paint("value", true),
                format!("\x1b[{code}mvalue\x1b[0m")
            );
            assert_eq!(tone.paint("value", false), "value");
        }
        assert!(color_allowed(true, None, Some("xterm")));
        assert!(color_allowed(true, Some(""), None));
        assert!(!color_allowed(false, None, Some("xterm")));
        assert!(!color_allowed(true, Some("1"), Some("xterm")));
        assert!(!color_allowed(true, None, Some("dumb")));
    }
}
