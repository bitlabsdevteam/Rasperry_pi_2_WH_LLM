use ai_assistant::{
    adapters::storage::SqliteStore,
    config::{AssistantPaths, LlmConfig, MemoryConfig},
    core::{context::maybe_compact, memory::record_turn},
    util::unique_temp_dir,
};

#[test]
fn context_compaction_triggers_after_threshold() {
    let root = unique_temp_dir("assistant-context-test");
    let paths = AssistantPaths::new(root);
    paths.ensure_defaults().unwrap();
    let store = SqliteStore::new(&paths).unwrap();
    let config = MemoryConfig {
        recent_turn_limit: 6,
        compact_after_turns: 6,
        retain_recent_turns: 4,
        token_budget: 128,
        compact_context_threshold_percent: 70,
        memory_search_limit: 4,
        memory_ttl_days: 30,
    };
    let llm = LlmConfig {
        prefer_http: false,
        endpoint: String::new(),
        health_endpoint: String::new(),
        model: "mock".into(),
        binary_path: String::new(),
        model_path: String::new(),
        threads: 1,
        context_size: 1024,
        predict_tokens: 64,
        timeout_secs: 1,
        retries: 0,
        stream: false,
    };

    for index in 0..6 {
        record_turn(&paths, &store, "default", "user", &format!("user {index}")).unwrap();
    }

    let outcome = maybe_compact(&paths, &store, "default", &config, &llm).unwrap();
    assert!(outcome.unwrap().contains("compacted session"));
}
