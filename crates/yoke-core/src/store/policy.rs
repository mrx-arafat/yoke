//! Immutable configuration provenance. These archives explain decisions; they never grant
//! executable trust. Only the current trust table can do that.
use super::{Store, TrustRow};
use crate::config::LoadedConfig;
use anyhow::{ensure, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize, Serialize)]
pub struct PolicyVersion {
    pub yoke_version: String,
    pub config: Value,
    pub sources: Vec<TrustRow>,
}

fn digest(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

impl Store {
    /// Capture the configuration actually loaded, even if a human has trusted new bytes
    /// since. Read the archive first so ordinary hooks need no extra write transaction.
    /// Like executable trust, archived source text is exact and is never redacted.
    pub fn record_policy_version(&self, loaded: &LoadedConfig) -> Result<String> {
        let content = serde_json::to_string(&PolicyVersion {
            yoke_version: env!("CARGO_PKG_VERSION").into(),
            config: serde_json::to_value(&loaded.config)?,
            sources: loaded.policy_sources.clone(),
        })?;
        let hash = digest(&content);
        let existing: Option<String> = self
            .connection
            .query_row(
                "SELECT content FROM policy_versions WHERE sha256=?1",
                [&hash],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            ensure!(existing == content, "archived policy hash mismatch; keep state.db and its sidecars together, collect yoke --version and this diagnostic, and report the damaged archive; do not edit its hash or trust rows");
        } else {
            self.connection.execute(
                "INSERT OR IGNORE INTO policy_versions(sha256,content) VALUES (?1,?2)",
                params![hash, content],
            )?;
        }
        Ok(hash)
    }

    /// Lookup by recorded hash only. Never read a request-supplied path or today's trust.
    pub fn policy_version(&self, hash: &str) -> Result<Option<PolicyVersion>> {
        let content: Option<String> = self
            .connection
            .query_row(
                "SELECT content FROM policy_versions WHERE sha256=?1",
                [hash],
                |row| row.get(0),
            )
            .optional()?;
        content
            .map(|content| {
                ensure!(digest(&content) == hash, "archived policy hash mismatch; keep state.db and its sidecars together, collect yoke --version and this diagnostic, and report the damaged archive; do not edit its hash or trust rows");
                Ok(serde_json::from_str(&content)?)
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config, trust};

    #[test]
    fn policy_archive_keeps_loaded_sources_after_retrust_and_detects_damage() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let home = dir.path().join("home");
        let project = dir.path().join("project");
        std::fs::create_dir_all(home.join(".yoke"))?;
        std::fs::create_dir_all(&project)?;
        let path = home.join(".yoke/state.db");
        let store = Store::open(&path)?;
        let global = "[stop]\nmax_blocks=3\n";
        let original = "mode='enforce'\n# first version\n";
        std::fs::write(home.join(".yoke/config.toml"), global)?;
        trust::prepare_global(&store, &home)?.commit(&store)?;
        std::fs::write(project.join("yoke.toml"), original)?;
        trust::prepare_project(&store, &project)?.commit(&store)?;
        let loaded = config::load_trusted(&store, &project, &home)?;
        std::fs::write(project.join("yoke.toml"), "mode='observe'\n")?;
        trust::prepare_project(&store, &project)?.commit(&store)?;
        // Archive after the change: re-reading the trust table here would misattribute it.
        let hash = store.record_policy_version(&loaded)?;
        let archived = store.policy_version(&hash)?.unwrap();
        assert_eq!(archived.config["mode"], "enforce");
        assert_eq!(archived.config["stop"]["max_blocks"], 3);
        assert_eq!(archived.sources.len(), 2);
        assert_eq!(archived.sources[0].content, global);
        assert_eq!(archived.sources[1].content, original);
        assert_eq!(archived.sources[1].sha256, digest(original));
        // Repeated hooks, including read-only fallback, reuse the same archived bytes.
        let reader = Store::open_read_only(&path)?;
        assert_eq!(reader.record_policy_version(&loaded)?, hash);
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM policy_versions", [], |r| r
                    .get::<_, i64>(0))?,
            1
        );
        let current = config::load_trusted(&store, &project, &home)?;
        assert_ne!(store.record_policy_version(&current)?, hash);
        store.connection.execute(
            "UPDATE policy_versions SET content='{}' WHERE sha256=?1",
            [&hash],
        )?;
        for error in [
            store.policy_version(&hash).err().unwrap(),
            store.record_policy_version(&loaded).err().unwrap(),
        ] {
            let message = error.to_string();
            assert!(
                message.contains("keep state.db and its sidecars"),
                "{message}"
            );
            assert!(message.contains("yoke --version"), "{message}");
        }
        Ok(())
    }

    #[test]
    fn policy_archive_distinguishes_defaults_from_unavailable_provenance() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(":memory:")?;
        std::fs::write(dir.path().join("yoke.toml"), "mode='enforce'\n# ignored\n")?;
        let loaded = config::load_trusted(&store, dir.path(), dir.path())?;
        let hash = store.record_policy_version(&loaded)?;
        let archived = store.policy_version(&hash)?.unwrap();
        assert!(archived.sources.is_empty());
        assert_eq!(archived.config["mode"], "observe");
        assert_eq!(archived.yoke_version, env!("CARGO_PKG_VERSION"));
        assert!(store.policy_version(&"0".repeat(64))?.is_none());
        Ok(())
    }
}
