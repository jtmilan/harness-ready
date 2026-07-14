//! Headless model-alias resolution.
//!
//! EXTRACTED VERBATIM (Phase 0) from agent-teams `app/src-tauri/src/lib.rs` (~L3821).
//! Kept byte-for-byte so the extracted behavior is provably identical; the two unit
//! tests below are also lifted verbatim (lib.rs ~L15572 / ~L15603).

use std::path::Path;

/// Map the app's 1P model aliases to the cwd repo's configured Bedrock ids when
/// `CLAUDE_CODE_USE_BEDROCK` is set in `<cwd>/.claude/settings.local.json`
/// (else the headless synthesis 400s with "provided model identifier is invalid").
/// The matching tier is chosen by substring. On any miss → passthrough (never invent an id).
pub fn resolve_headless_model(cwd: &Path, requested: &str) -> String {
    let settings = cwd.join(".claude").join("settings.local.json");
    let Ok(body) = std::fs::read_to_string(&settings) else {
        return requested.to_string();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return requested.to_string();
    };
    let Some(env) = v.get("env").and_then(|e| e.as_object()) else {
        return requested.to_string();
    };
    let bedrock = env
        .get("CLAUDE_CODE_USE_BEDROCK")
        .map(|x| x.as_str() == Some("1") || x.as_bool() == Some(true))
        .unwrap_or(false);
    if !bedrock {
        return requested.to_string();
    }
    let key = if requested.contains("haiku") {
        "ANTHROPIC_DEFAULT_HAIKU_MODEL"
    } else if requested.contains("opus") {
        "ANTHROPIC_DEFAULT_OPUS_MODEL"
    } else if requested.contains("sonnet") {
        "ANTHROPIC_DEFAULT_SONNET_MODEL"
    } else {
        return requested.to_string();
    };
    env.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| requested.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bedrock-alias fix: maps the app's 1P aliases to the cwd repo's configured Bedrock ids
    // when CLAUDE_CODE_USE_BEDROCK is set; the matching tier is chosen by substring.
    #[test]
    fn resolve_headless_model_maps_alias_on_bedrock_repo() {
        let root = std::env::temp_dir().join(format!("at-bedrock-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(".claude/settings.local.json"),
            r#"{"env":{"CLAUDE_CODE_USE_BEDROCK":"1",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL":"us.anthropic.claude-haiku-4-5-20251001-v1:0",
                "ANTHROPIC_DEFAULT_OPUS_MODEL":"us.anthropic.claude-opus-4-8[1m]",
                "ANTHROPIC_DEFAULT_SONNET_MODEL":"us.anthropic.claude-sonnet-4-5-20250929-v1:0[1m]"}}"#,
        )
        .unwrap();

        assert_eq!(
            resolve_headless_model(&root, "claude-haiku-4-5"),
            "us.anthropic.claude-haiku-4-5-20251001-v1:0"
        );
        assert_eq!(
            resolve_headless_model(&root, "claude-opus-4-8"),
            "us.anthropic.claude-opus-4-8[1m]"
        );
        assert_eq!(
            resolve_headless_model(&root, "claude-sonnet-4-6"),
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0[1m]"
        );
        // a model with no tier token is left untouched
        assert_eq!(
            resolve_headless_model(&root, "some-other-model"),
            "some-other-model"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // Passthrough: non-Bedrock repo, no settings file, or a missing matching default → unchanged.
    #[test]
    fn resolve_headless_model_passthrough_when_not_bedrock() {
        let root = std::env::temp_dir().join(format!("at-nobedrock-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // no settings file at all
        assert_eq!(
            resolve_headless_model(&root, "claude-haiku-4-5"),
            "claude-haiku-4-5"
        );

        // settings present but Bedrock OFF
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(".claude/settings.local.json"),
            r#"{"env":{"ANTHROPIC_DEFAULT_HAIKU_MODEL":"us.anthropic.claude-haiku-4-5-20251001-v1:0"}}"#,
        )
        .unwrap();
        assert_eq!(
            resolve_headless_model(&root, "claude-haiku-4-5"),
            "claude-haiku-4-5"
        );

        // Bedrock ON but the matching default is absent → passthrough (don't fabricate an id)
        std::fs::write(
            root.join(".claude/settings.local.json"),
            r#"{"env":{"CLAUDE_CODE_USE_BEDROCK":"1"}}"#,
        )
        .unwrap();
        assert_eq!(
            resolve_headless_model(&root, "claude-opus-4-8"),
            "claude-opus-4-8"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
