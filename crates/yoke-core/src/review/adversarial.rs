//! Deterministic hostile-input coverage through the public policy boundaries.

use crate::{config::Config, event::AgentEvent, event::EventKind, guard, redact};
use serde_json::json;

fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    *seed >> 32
}

#[test]
fn generated_shell_word_spellings_cannot_hide_human_only_commands() {
    let home = tempfile::tempdir().unwrap();
    let mut seed = 0x435833;
    for _ in 0..128 {
        let mut command = String::new();
        for word in ["yoke", "trust"] {
            if !command.is_empty() {
                command.push_str(if next(&mut seed) & 1 == 0 { "\t" } else { " " });
            }
            for letter in word.chars() {
                match next(&mut seed) % 4 {
                    0 => command.push(letter),
                    1 => command.push_str(&format!("'{letter}'")),
                    2 => command.push_str(&format!("\"{letter}\"")),
                    _ => command.push_str(&format!("\\{letter}")),
                }
            }
        }
        let event = AgentEvent {
            tool_name: Some("Bash".into()),
            tool_input: json!({"command": command}),
            ..Default::default()
        };
        assert!(
            guard::evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_some(),
            "{event:?}"
        );
    }
}

#[test]
fn generated_valid_policies_roundtrip_without_changing_gate_behavior() {
    for index in 0..64 {
        let source = format!("mode = 'enforce'\n[[gate]]\nname = 'deny-{index}'\non = 'pre_tool_use'\naction = 'deny'\nmatch_contains = 'blocked-{index}'\nmessage = 'declined-{index}'\n");
        let parsed = Config::parse(&source).unwrap();
        let encoded = toml::to_string(&parsed).unwrap();
        let roundtrip = Config::parse(&encoded).unwrap();
        assert_eq!(roundtrip.mode.as_str(), "enforce");
        assert_eq!(roundtrip.gates.len(), 1);
        assert_eq!(roundtrip.gates[0].name, format!("deny-{index}"));
        assert_eq!(
            roundtrip.gates[0].match_contains.as_deref(),
            Some(format!("blocked-{index}").as_str())
        );
        assert_eq!(
            roundtrip.gates[0].message.as_deref(),
            Some(format!("declined-{index}").as_str())
        );
    }
}

#[test]
fn hostile_configuration_and_hook_corpus_rejects_invalid_input_without_panicking() {
    let nested = format!("mode = {}'enforce'{}", "[".repeat(256), "]".repeat(256));
    let huge = format!("mode = '{}'", "x".repeat(128 * 1024));
    for source in [
        "mode = 'allow'",
        "mode = 'enforce'\0",
        "mode = 'observe'\nmode = 'enforce'",
        &nested,
        &huge,
    ] {
        assert!(Config::parse(source).is_err(), "hostile policy accepted");
    }
    let deep_json = format!("{{\"tool_input\":{}0{}}}", "[".repeat(256), "]".repeat(256));
    for payload in [
        b"\xff".as_slice(),
        b"{}\0",
        b"{\"tool_input\":",
        deep_json.as_bytes(),
    ] {
        assert!(
            serde_json::from_slice::<AgentEvent>(payload).is_err(),
            "malformed hook accepted"
        );
    }
    let home = tempfile::tempdir().unwrap();
    let event: AgentEvent = serde_json::from_value(json!({"tool_name": "Bash", "tool_input": {"command": format!("{}; yoke trust", "x".repeat(128 * 1024))}})).unwrap();
    assert!(guard::evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_some());
}

#[test]
fn generated_text_never_retains_an_embedded_supported_secret() {
    let secrets = [
        ["ghp", "_CX3SyntheticSecret01234567890123456789"].concat(),
        ["sk", "_live_CX3SyntheticSecret0123456789"].concat(),
        ["AIza", "CX3SyntheticSecret01234567890123456789"].concat(),
        ["npm", "_CX3SyntheticSecret01234567890123456789"].concat(),
    ];
    let alphabet = ['a', '9', ' ', '\n', 'λ', '中', '\0', '\'', '"', '/', ':'];
    let mut seed = 0x534543524554;
    for _ in 0..128 {
        let prefix: String = (0..32)
            .map(|_| alphabet[(next(&mut seed) as usize) % alphabet.len()])
            .collect();
        let suffix: String = (0..32)
            .map(|_| alphabet[(next(&mut seed) as usize) % alphabet.len()])
            .collect();
        for secret in &secrets {
            let input = format!("{prefix}{secret}{suffix}");
            assert!(
                !redact::redact(&input).contains(secret),
                "secret survived: {input:?}"
            );
        }
    }
}
