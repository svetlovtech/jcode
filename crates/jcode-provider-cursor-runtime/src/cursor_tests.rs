use super::*;
use std::io::Read;

#[test]
fn failed_catalog_refresh_returns_error_and_preserves_models() {
    let _guard = jcode_base::storage::lock_test_env();
    let previous = std::env::var_os("CURSOR_API_KEY");
    jcode_base::env::set_var("CURSOR_API_KEY", "synthetic-test-key");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        let mut provider = CursorCliProvider::new();
        provider.client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
            .timeout(std::time::Duration::from_millis(500))
            .build()
            .unwrap();
        *provider.fetched_models.write().unwrap() = vec!["cached-cursor-model".into()];
        let result = provider.refresh_model_catalog().await;
        assert!(
            provider
                .available_models_display()
                .contains(&"cached-cursor-model".into())
        );
        result
    });
    match previous {
        Some(value) => jcode_base::env::set_var("CURSOR_API_KEY", value),
        None => jcode_base::env::remove_var("CURSOR_API_KEY"),
    }
    assert!(result.is_err());
}

#[test]
fn routed_prompt_includes_system_and_namespaced_tool_directive() {
    let tools = vec![ToolDefinition {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        defer_loading: false,
    }];
    let prompt = agent_transport::routed_prompt("Read notes.txt", "Be precise.", &tools);
    assert!(prompt.contains("Be precise."));
    assert!(prompt.contains("cc_read"));
    assert!(prompt.contains("no built-in tools"));
    assert!(prompt.ends_with("Read notes.txt"));
}

#[test]
fn summary_compaction_stays_disabled_until_prompt_truncation_is_safe() {
    let provider = CursorCliProvider::new();
    assert!(!provider.supports_compaction());
    assert!(!provider.uses_jcode_compaction());
}

#[test]
fn available_models_include_composer_models() {
    let provider = CursorCliProvider::new();
    let models = provider.available_models();
    assert!(models.contains(&"composer-2"));
    assert!(models.contains(&"composer-2.5"));
}

#[test]
fn available_models_display_includes_custom_current_model() {
    let provider = CursorCliProvider::new();
    provider.set_model("future-cursor-model").unwrap();

    let models = provider.available_models_display();
    assert!(models.contains(&"future-cursor-model".to_string()));
}

#[test]
fn available_models_display_prefers_fetched_cursor_models() {
    let provider = CursorCliProvider::new();
    *provider.fetched_models.write().unwrap() = vec![
        "claude-4-sonnet-thinking".to_string(),
        "gpt-5.2".to_string(),
    ];

    let models = provider.available_models_display();
    assert_eq!(
        models.first().map(|model| model.as_str()),
        Some("claude-4-sonnet-thinking")
    );
    assert!(models.iter().any(|model| model == "gpt-5.2"));
    assert!(models.iter().any(|model| model == "composer-2.5"));
}

#[test]
fn merge_cursor_models_deduplicates_dynamic_entries() {
    let models = merge_cursor_models(
        &[
            "composer-2".to_string(),
            "claude-4-sonnet-thinking".to_string(),
            "claude-4-sonnet-thinking".to_string(),
        ],
        "claude-4-sonnet-thinking",
    );

    assert_eq!(
        models
            .iter()
            .filter(|model| model.as_str() == "claude-4-sonnet-thinking")
            .count(),
        1
    );
    assert!(models.iter().any(|model| model == "composer-2"));
}

#[test]
fn available_models_display_seeds_from_persisted_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    jcode_base::env::set_var("JCODE_HOME", temp.path());

    let path = CursorCliProvider::persisted_catalog_path().expect("catalog path");
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec!["cursor-disk-model".to_string()],
            fetched_at_rfc3339: chrono::Utc::now().to_rfc3339(),
        },
    )
    .expect("write persisted catalog");

    let provider = CursorCliProvider::new();
    assert!(
        provider
            .available_models_display()
            .contains(&"cursor-disk-model".to_string())
    );

    if let Some(prev_home) = prev_home {
        jcode_base::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_base::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn set_model_accepts_composer_models() {
    let provider = CursorCliProvider::new();

    provider.set_model("composer-2").unwrap();
    assert_eq!(provider.model(), "composer-2");

    provider.set_model("composer-2.5").unwrap();
    assert_eq!(provider.model(), "composer-2.5");
}

#[test]
fn runtime_cursor_api_key_reads_env() {
    let previous = std::env::var_os("CURSOR_API_KEY");
    jcode_base::env::set_var("CURSOR_API_KEY", "cursor-env-test");

    assert_eq!(runtime_cursor_api_key().as_deref(), Some("cursor-env-test"));

    if let Some(previous) = previous {
        jcode_base::env::set_var("CURSOR_API_KEY", previous);
    } else {
        jcode_base::env::remove_var("CURSOR_API_KEY");
    }
}
