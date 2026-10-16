//! The starter `yoke.toml` that `yoke init` writes.

const TEMPLATE: &str = include_str!("../starter.toml");

/// The template with `[meta] name` set to the project directory instead of a placeholder.
pub fn render(project_name: &str) -> String {
    let quoted = toml_edit::Value::from(project_name).to_string();
    TEMPLATE.replacen("\"my-project\"", quoted.trim(), 1)
}

#[cfg(test)]
mod tests {
    use super::render;

    fn parsed_name(text: &str) -> String {
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        doc["meta"]["name"].as_str().unwrap().to_owned()
    }

    #[test]
    fn the_starter_is_named_after_the_directory_and_stays_valid_toml() {
        assert_eq!(parsed_name(&render("billing-api")), "billing-api");
        // Quotes and backslashes in a directory name must not break the file.
        assert_eq!(parsed_name(&render("a\"b\\c")), "a\"b\\c");
        let text = render("billing-api");
        assert!(!text.contains("my-project"), "{text}");
        assert!(text.contains("# [memory]"), "{text}");
    }
}
