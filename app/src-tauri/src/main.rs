// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Hold the Sentry guard for the whole process lifetime (Drop flushes pending events on exit).
    // `None` (no DSN baked at build) → Sentry is inert. Bound to `_sentry` (NOT `_`) so it is
    // dropped at the END of main, not immediately.
    let _sentry = app_lib::init_sentry();
    app_lib::run()
}
