use ai_assistant::{
    adapters::storage::SqliteStore,
    config::{AssistantPaths, MemoryConfig, SchedulerConfig},
    core::{
        context::maybe_compact,
        memory::record_turn,
        scheduler::{add_job, run_due_jobs},
        tools::ToolExecutor,
    },
    util::unique_temp_dir,
};

#[test]
fn long_conversation_stress_keeps_recent_context() {
    let root = unique_temp_dir("assistant-stress-long");
    let paths = AssistantPaths::new(root);
    paths.ensure_defaults().unwrap();
    let store = SqliteStore::new(&paths).unwrap();
    let memory = MemoryConfig {
        recent_turn_limit: 8,
        compact_after_turns: 20,
        retain_recent_turns: 6,
        token_budget: 160,
        memory_search_limit: 4,
        memory_ttl_days: 30,
    };

    for turn in 0..24 {
        record_turn(&paths, &store, "default", "user", &format!("turn {turn}")).unwrap();
    }

    let outcome = maybe_compact(&paths, &store, "default", &memory).unwrap();
    assert!(outcome.unwrap().contains("compacted"));
}

#[test]
fn low_memory_simulation_uses_small_token_budget() {
    let root = unique_temp_dir("assistant-stress-lowmem");
    let paths = AssistantPaths::new(root);
    paths.ensure_defaults().unwrap();
    let store = SqliteStore::new(&paths).unwrap();
    let memory = MemoryConfig {
        recent_turn_limit: 2,
        compact_after_turns: 4,
        retain_recent_turns: 2,
        token_budget: 48,
        memory_search_limit: 2,
        memory_ttl_days: 30,
    };

    for turn in 0..4 {
        record_turn(
            &paths,
            &store,
            "default",
            "assistant",
            &format!("state {turn}"),
        )
        .unwrap();
    }

    assert!(
        maybe_compact(&paths, &store, "default", &memory)
            .unwrap()
            .is_some()
    );
}

#[test]
fn multiple_scheduled_jobs_run_in_single_tick() {
    let root = unique_temp_dir("assistant-stress-jobs");
    let paths = AssistantPaths::new(root);
    paths.ensure_defaults().unwrap();
    let store = SqliteStore::new(&paths).unwrap();
    let scheduler = SchedulerConfig {
        poll_seconds: 1,
        max_jobs_per_tick: 8,
        allow_shell_jobs: false,
    };
    let tools = ToolExecutor::new(vec!["echo".into()], paths.root.clone());

    add_job(&store, "job-one", 0, "task add first").unwrap();
    add_job(&store, "job-two", 0, "task add second").unwrap();
    add_job(&store, "job-three", 0, "summarize").unwrap();

    let logs = run_due_jobs(&paths, &store, &scheduler, &tools).unwrap();
    assert_eq!(logs.len(), 3);
}
