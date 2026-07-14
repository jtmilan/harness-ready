fn main() {
    // Rebuild when the baked Sentry DSN changes (option_env! in lib.rs::init_sentry) so a
    // changed/added DSN actually re-bakes instead of using a stale cached compile.
    println!("cargo:rerun-if-env-changed=AGENT_TEAMS_SENTRY_DSN");
    tauri_build::build()
}
