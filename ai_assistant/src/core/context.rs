use crate::{
    adapters::storage::SqliteStore,
    config::{AssistantPaths, LlmConfig, MemoryConfig},
    core::memory::{compact_session, session_token_estimate, turn_count},
};

pub fn maybe_compact(
    paths: &AssistantPaths,
    store: &SqliteStore,
    session_id: &str,
    config: &MemoryConfig,
    llm: &LlmConfig,
) -> Result<Option<String>, String> {
    let turn_count = turn_count(store, session_id)?;
    let estimated_tokens = session_token_estimate(store, session_id)?;
    if !should_compact(turn_count, estimated_tokens, config, llm) {
        return Ok(None);
    }
    Ok(Some(compact_session(
        paths,
        store,
        session_id,
        config.retain_recent_turns,
    )?))
}

fn should_compact(
    turn_count: usize,
    estimated_tokens: usize,
    config: &MemoryConfig,
    llm: &LlmConfig,
) -> bool {
    if turn_count >= config.compact_after_turns {
        return true;
    }

    let threshold = context_compaction_threshold(config, llm);
    estimated_tokens >= threshold
}

pub fn context_compaction_threshold(config: &MemoryConfig, llm: &LlmConfig) -> usize {
    let available_prompt_tokens = llm
        .context_size
        .saturating_sub(llm.predict_tokens)
        .saturating_sub(96)
        .max(128);
    let clamped_percent = config.compact_context_threshold_percent.clamp(1, 100);
    let threshold = (available_prompt_tokens * clamped_percent) / 100;
    threshold.max(1)
}

#[cfg(test)]
mod tests {
    use crate::config::{LlmConfig, MemoryConfig};

    use super::{context_compaction_threshold, should_compact};

    #[test]
    fn compaction_threshold_uses_seventy_percent_of_available_prompt_budget() {
        let threshold = context_compaction_threshold(
            &MemoryConfig {
                recent_turn_limit: 8,
                compact_after_turns: 12,
                retain_recent_turns: 6,
                token_budget: 2048,
                compact_context_threshold_percent: 70,
                memory_search_limit: 6,
                memory_ttl_days: 30,
            },
            &LlmConfig {
                prefer_http: false,
                endpoint: String::new(),
                health_endpoint: String::new(),
                model: "mock".into(),
                binary_path: String::new(),
                model_path: String::new(),
                threads: 1,
                context_size: 4096,
                predict_tokens: 512,
                timeout_secs: 1,
                retries: 0,
                stream: false,
            },
        );

        assert_eq!(threshold, 2441);
    }

    #[test]
    fn compaction_can_trigger_from_context_usage_before_turn_limit() {
        let memory = MemoryConfig {
            recent_turn_limit: 8,
            compact_after_turns: 20,
            retain_recent_turns: 6,
            token_budget: 2048,
            compact_context_threshold_percent: 70,
            memory_search_limit: 6,
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

        assert!(should_compact(4, 700, &memory, &llm));
        assert!(!should_compact(4, 100, &memory, &llm));
    }
}
