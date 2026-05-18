use ai_assistant::{
    adapters::storage::SqliteStore,
    config::{AssistantPaths, MemoryConfig},
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
        memory_search_limit: 4,
        memory_ttl_days: 30,
    };

    for index in 0..6 {
        record_turn(&paths, &store, "default", "user", &format!("user {index}")).unwrap();
    }

    let outcome = maybe_compact(&paths, &store, "default", &config).unwrap();
    assert!(outcome.unwrap().contains("compacted session"));
}
