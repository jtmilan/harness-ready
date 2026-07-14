use std::fs;
use std::path::PathBuf;

fn app_source(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[test]
fn ac1_dispatch_mints_a_run_directory_before_fanout() {
    let js = app_source("../src/main.js");

    assert!(
        js.contains(r#"invoke("bridge_new_run", { repo, label: bridgeRunLabel() })"#),
        "dispatchBridge must mint a run directory before sending pane work"
    );
    // ADE slice 2: the report wording moved to the core/roles section library; main.js now
    // fetches the pack once per dispatch batch and splices the per-pane result path between
    // report_head and report_tail. Assert BOTH halves of the seam: the JS splice + the
    // library wording (via the roles API the app composes from).
    assert!(
        js.contains(r#"invoke("role_prompt_sections""#),
        "dispatch must fetch the role-prompt section pack (core/roles library)"
    );
    assert!(
        js.contains("SEC.report_head + bridgeRunDir + '/' + d.id + '.md' + SEC.report_tail"),
        "each dispatched pane must receive an exact per-pane result path"
    );
    assert!(
        roles::REPORT_WRITE_HEAD.contains("write your COMPLETE result as Markdown"),
        "the library write head must carry the report-write instruction"
    );
    // 07-01: authoritative dispatched-id manifest + structured pane-report instruction
    assert!(
        js.contains(r#"invoke("bridge_write_manifest""#),
        "dispatch must write the authoritative dispatched-id manifest (P2-B)"
    );
    let contract = roles::prompt_section(roles::PromptRole::Worker, roles::PromptSection::ReportFormat);
    assert!(
        contract.contains("## CONTRACT") && contract.contains("## UNVERIFIED"),
        "the pane-report instruction must require the structured verification sections (P2-A)"
    );
}

#[test]
fn ac2_dispatched_run_survives_bridge_modal_close() {
    let js = app_source("../src/main.js");

    // The run-metadata localStorage key is routed through the LS_KEYS map
    // (LS_KEYS.bridgeRun === "at_bridge_run"); assert the indirected form + the
    // key binding rather than an inline literal.
    assert!(
        js.contains(r#"bridgeRun: "at_bridge_run""#),
        "LS_KEYS.bridgeRun must map to the at_bridge_run storage key"
    );
    assert!(
        js.contains("localStorage.setItem(LS_KEYS.bridgeRun"),
        "fan-in run metadata must be persisted while agents are working"
    );
    assert!(
        js.contains("function restoreBridgeRun()")
            && js.contains("localStorage.getItem(LS_KEYS.bridgeRun)"),
        "Bridge reopen must restore an in-flight run"
    );
    assert!(
        js.contains("localStorage.removeItem(LS_KEYS.bridgeRun)"),
        "completed synthesis must clear stale run metadata"
    );
}

fn admit_pending_body(js: &str) -> Option<&str> {
    let start = js.find("function admitPending")?;
    let rest = &js[start..];
    // Next top-level `function ` after the opening — crude but stable for this smoke file.
    let end = rest[1..]
        .find("\nfunction ")
        .map(|i| start + 1 + i)
        .unwrap_or(js.len());
    Some(&js[start..end])
}

#[test]
fn p4_scheduler_admit_event_does_not_silently_drop_missing_pending_record() {
    let js = app_source("../src/main.js");

    // P4 found a fan-in risk in the scheduler UI lane: if the webview reloads
    // while the backend still holds pending work, `workspace-admitted` can arrive
    // after the transient frontend `pending` map is empty. Once that UI exists,
    // this guard requires a recovery path instead of a bare early return.
    let Some(body) = admit_pending_body(&js) else {
        return;
    };

    assert!(
        js.contains(r#"tauriEvent.listen("workspace-admitted""#)
            || js.contains(r#".listen("workspace-admitted""#),
        "scheduler UI must listen for backend admission events"
    );
    assert!(
        !body.contains("if (!rec) return;"),
        "admitPending must recover or refresh when pending[id] is missing, not silently drop the admitted live pane"
    );
    assert!(
        body.contains("ensureSession(id)") || body.contains("list_workspaces") || body.contains("restore"),
        "missing pending[id] should still attach or refresh the now-live workspace"
    );
}

#[test]
fn ac3_fanin_reads_agent_markdown_and_writes_final_markdown() {
    let rs = app_source("src/lib.rs");

    assert!(
        rs.contains("fn bridge_synthesize") && rs.contains(r#"extension().and_then(|x| x.to_str()) == Some("md")"#),
        "bridge_synthesize must read markdown agent outputs"
    );
    assert!(
        rs.contains(r#"file_name().and_then(|x| x.to_str()) != Some("final.md")"#),
        "bridge_synthesize must not feed a previous final.md back into synthesis"
    );
    // P1 consolidation (supersedes the old inline `route_synthesis`): the hardcoded `final.md`
    // write was lifted into the ONE shared `synthesize_core` (core/flywheel), which owns the
    // verdict-gated write (Pass → final.md; Hold/Reject → final.HELD.md). bridge_synthesize now
    // routes through it rather than writing inline.
    assert!(
        rs.contains("synthesize_core(") && rs.contains(r#"BridgeVerdict::Pass => ("pass", "PASS")"#),
        "fan-in must route through the shared synthesize_core with a Pass verdict projection"
    );
    // The Pass→final.md / Reject→final.HELD.md routing stays behaviorally covered (the write moved
    // to synthesize_core, so this asserts the coverage still lives in the app crate rather than a
    // now-deleted inline string).
    assert!(
        rs.contains(r#"run.join("final.md"), "Pass routes to final.md""#)
            && rs.contains(r#"run2.join("final.HELD.md"), "Reject routes to final.HELD.md""#),
        "synthesize_core's verdict routing (Pass→final.md, Reject→final.HELD.md) must stay covered"
    );
}

#[test]
fn ac4_run_labels_are_sanitized_before_becoming_path_segments() {
    let rs = app_source("src/lib.rs");

    assert!(
        rs.contains("fn sanitize_run_label") && rs.contains("is_ascii_alphanumeric()"),
        "run labels must be sanitized before filesystem use"
    );
    assert!(
        rs.contains(r#"trim_matches('-')"#) && rs.contains(r#""run".to_string()"#),
        "empty or punctuation-only labels must fall back to a safe segment"
    );
}

#[test]
fn ac5_runs_jsonl_is_a_sibling_of_state_root() {
    let rs = app_source("src/lib.rs");

    assert!(
        rs.contains("runs.jsonl"),
        "expected a durable runs.jsonl registry for fan-in runs"
    );
    assert!(
        rs.contains("default_state_root()") && rs.contains(".parent()") && rs.contains("runs.jsonl"),
        "runs.jsonl must be resolved as a sibling of state_root so startup wipes do not delete it"
    );
}
