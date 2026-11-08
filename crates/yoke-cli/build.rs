//! Embeds the repository's own documentation (MANUAL.md and docs/*.md) for the console's
//! Documentation page. A build from a packaged crate has no repository around it, so the list is
//! then empty and the page says where to read the documentation instead of failing the build.
use std::{env, fs, path::Path};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest).join("../..");
    let mut files: Vec<String> = Vec::new();
    if root.join("MANUAL.md").is_file() {
        files.push("MANUAL.md".to_owned());
    }
    if let Ok(read) = fs::read_dir(root.join("docs")) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".md") && entry.path().is_file() {
                files.push(format!("docs/{name}"));
            }
        }
    }
    files.sort();
    let mut source = String::from("static EMBEDDED: &[(&str, &str)] = &[\n");
    for file in &files {
        let absolute = root.join(file);
        let absolute = fs::canonicalize(&absolute).unwrap_or(absolute);
        source.push_str(&format!(
            "    ({file:?}, include_str!({:?})),\n",
            absolute.display().to_string()
        ));
    }
    source.push_str("];\n");
    let out = Path::new(&env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("embedded_docs.rs");
    fs::write(out, source).expect("write embedded_docs.rs");
    if root.join("docs").is_dir() {
        println!("cargo:rerun-if-changed=../../docs");
        println!("cargo:rerun-if-changed=../../MANUAL.md");
    } else {
        println!("cargo:rerun-if-changed=build.rs");
    }
}
