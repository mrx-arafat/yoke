//! Read-only comparison of receipted hooks with what this executable would install now.

use std::path::PathBuf;

use anyhow::Result;

use super::{
    files, hook_group, read_receipt, settings, unlabeled_group, Agent, InstallOptions,
    COMMON_EVENTS,
};

pub(crate) struct Managed {
    pub(crate) settings: PathBuf,
    /// Handler commands the installation receipt owns in `settings`.
    pub(crate) commands: Vec<String>,
    /// `(name, explanation)` for each managed definition a fresh install would differ from.
    pub(crate) stale: Vec<(String, String)>,
}

pub(crate) fn managed(options: &InstallOptions, agent: Agent) -> Result<Option<Managed>> {
    let Some(receipt) = read_receipt(options, agent)? else {
        return Ok(None);
    };
    let mut stale = Vec::new();
    if receipt.executable != options.executable {
        stale.push((
            "executable".into(),
            format!(
                "hooks run {}, not this yoke at {}; yoke install refuses until yoke uninstall",
                receipt.executable.display(),
                options.executable.display()
            ),
        ));
    }
    let current = settings(files::read(&receipt.hooks.path)?.as_deref())?;
    let mut events = COMMON_EVENTS.to_vec();
    if agent == Agent::Claude {
        events.extend(["Notification", "PostToolUseFailure"]);
    }
    let mut missing = Vec::new();
    for event in &events {
        let group = hook_group(&receipt.executable, agent, event)?;
        let groups = current["hooks"][event].as_array();
        if groups.is_some_and(|groups| groups.contains(&group)) {
            continue;
        }
        if groups.is_some_and(|groups| groups.contains(&unlabeled_group(&group))) {
            stale.push((
                (*event).into(),
                "unlabeled managed hook; yoke install relabels it".into(),
            ));
        } else {
            missing.push(*event);
        }
    }
    let advice = "run yoke install --dry-run to inspect, then yoke uninstall and yoke install";
    if missing.len() == events.len() {
        stale.push(("hooks".into(), format!("no managed hooks remain; {advice}")));
    } else {
        for event in missing {
            stale.push((
                event.into(),
                format!("managed hook missing or edited; {advice}"),
            ));
        }
    }
    let commands = receipt
        .added
        .iter()
        .filter_map(|owned| {
            owned.group["hooks"][0]["command"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    Ok(Some(Managed {
        settings: receipt.hooks.path,
        commands,
        stale,
    }))
}
