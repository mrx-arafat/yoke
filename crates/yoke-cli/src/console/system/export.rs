//! Export view: memory archives as browser downloads. Import stays a terminal command: the
//! console cannot receive an archive, and the terminal import maps every project explicitly.
use super::super::{
    render::{encode, escape},
    BadRequest, Console,
};
use super::{now, shell_quote, Download};
use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use yoke_core::{memory, store::Store};

pub(super) fn section(c: &Console) -> String {
    let project = c.project.display().to_string();
    let destination = shell_quote(&project);
    format!(
        r#"<section class="panel" aria-labelledby="sys-export"><div class="list-caption"><h2 id="sys-export">Export memory</h2></div><p>A portable archive of memories and the facts they were digested from, the same file <code>yoke memory export</code> writes. It is plaintext and may contain sensitive content; trusted policy is never included.</p><div class="actions"><a class="button" href="system/download?memory=project" download>Download this project's memory</a><a class="button quiet" href="system/download?memory=all" download>Download every project's memory</a></div><p class="muted">This project: <code>{}</code>. An archive holds at most 16 MiB or 20000 records; export one project at a time if a larger scope is refused.</p></section><section class="panel" aria-labelledby="sys-import"><div class="list-caption"><h2 id="sys-import">Import memory</h2></div><p>Importing runs in your own terminal, not here. An archive can hold up to 16 MiB of text that nobody in this project wrote, while console requests are limited to 16 KiB, so the console cannot receive it; reading a path typed into the browser would let the console open any file. The terminal import makes you name a destination for every project in the archive, shows the memories it would add before writing, unpins them unless you ask otherwise, and refuses agent processes.</p><p>Preview first, mapping each project in the archive (the error lists them) to this project or another:</p><pre><code>yoke memory import &lt;file&gt; --dry-run --map &lt;source-project&gt;={dest}</code></pre><p>Then import with the same mappings:</p><pre><code>yoke memory import &lt;file&gt; --map &lt;source-project&gt;={dest}</code></pre></section>"#,
        escape(&project),
        dest = escape(&destination),
    )
}

/// Write the archive into a private temporary folder under `~/.yoke` and hand it to the
/// download, which removes the folder once it is sent.
pub(super) fn download(store: &Store, c: &Console, scope: &str) -> Result<Download> {
    let project = match scope {
        "project" => Some(
            c.project
                .to_str()
                .context("The project path must be UTF-8 to export it")?,
        ),
        "all" => None,
        _ => return Err(BadRequest("Choose this project or every project.".into()).into()),
    };
    let folder = tempfile::Builder::new()
        .prefix(".export-")
        .tempdir_in(c.home.join(".yoke"))?;
    let path = folder.path().join("memory.json");
    memory::export_archive(store, &path, project, false)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let length = file.metadata()?.len();
    let stamp: String = yoke_core::timefmt::full_time_secs(now(), 0)
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    Ok(Download {
        file,
        length,
        name: format!("yoke-memory-{}-{stamp}.json", encode(scope)),
        kind: "application/json",
        _temporary: Some(folder),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Read;

    #[test]
    fn memory_exports_download_by_scope_and_import_stays_a_terminal_command() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let c = super::super::test_console(&root);
        let other = root.join("other");
        std::fs::create_dir(&other).unwrap();
        let store = Store::open(&c.database).unwrap();
        let mine = c.project.to_str().unwrap();
        memory::remember(&store, mine, "this project's note", false, 10).unwrap();
        memory::remember(
            &store,
            other.to_str().unwrap(),
            "other project's note",
            false,
            10,
        )
        .unwrap();

        let read = |scope: &str| {
            let params = BTreeMap::from([("memory".to_owned(), scope.to_owned())]);
            let mut download = super::super::download(&store, &c, &params).unwrap();
            assert_eq!(download.kind, "application/json");
            assert!(
                download.name.starts_with(&format!("yoke-memory-{scope}-")),
                "{}",
                download.name
            );
            let mut text = String::new();
            download.file.read_to_string(&mut text).unwrap();
            assert_eq!(text.len() as u64, download.length);
            text
        };
        let project = read("project");
        assert!(
            project.contains("this project's note") && !project.contains("other project's note")
        );
        assert!(read("all").contains("other project's note"));
        let bad = BTreeMap::from([("memory".to_owned(), "../x".to_owned())]);
        assert!(super::super::download(&store, &c, &bad).is_err());
        let leftovers: Vec<_> = std::fs::read_dir(c.home.join(".yoke"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".export-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary export folders are removed with the download"
        );

        let html = section(&c);
        assert!(html.contains("yoke memory import &lt;file&gt; --dry-run --map"));
        assert!(html.contains("href=\"system/download?memory=project\" download"));
    }
}
