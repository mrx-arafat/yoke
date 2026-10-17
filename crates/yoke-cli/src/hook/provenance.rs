//! OS-derived audit context, never an authentication claim or a payload-provided value.

use serde_json::Value;

pub(super) fn payload(event: &mut Value) -> String {
    // SAFETY: getppid takes no arguments and only reads the kernel's parent PID.
    let pid = unsafe { libc::getppid() };
    let parent = yoke_core::process::identity(pid);
    if let Some(fields) = event.as_object_mut() {
        fields.insert(
            "_yoke_parent".into(),
            serde_json::json!({"pid":pid,"name":parent.as_ref().map(|p| &p.name),"started":parent.as_ref().map(|p| &p.started),"parent_pid":parent.as_ref().map(|p| p.parent_pid)}),
        );
    }
    event.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn recorded_parent_overrides_payload_claim_with_os_provenance() {
        let raw = super::payload(
            &mut serde_json::json!({"_yoke_parent":{"pid":-1,"name":"forged"},"session_id":"s"}),
        );
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        // SAFETY: getppid has no arguments or side effects.
        assert_eq!(value["_yoke_parent"]["pid"], unsafe { libc::getppid() });
        assert_ne!(value["_yoke_parent"]["name"], "forged");
        assert_eq!(value["session_id"], "s");
        let mut event = serde_json::json!({"session_id":"s","tool_name":"Bash","tool_input":{"command":"true"}});
        let mut before = Vec::new();
        let mut after = Vec::new();
        for _ in 0..1000 {
            let start = std::time::Instant::now();
            std::hint::black_box(event.to_string());
            before.push(start.elapsed().as_nanos());
            let start = std::time::Instant::now();
            std::hint::black_box(super::payload(&mut event));
            after.push(start.elapsed().as_nanos());
            event.as_object_mut().unwrap().remove("_yoke_parent");
        }
        before.sort_unstable();
        after.sort_unstable();
        println!("parent capture microbenchmark, 1000 samples, baseline encoding p50={}ns p99={}ns; encoding+OS parent p50={}ns p99={}ns", before[500], before[990], after[500], after[990]);
    }
}
