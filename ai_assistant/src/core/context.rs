use crate::{
    adapters::storage::SqliteStore,
    config::{AssistantPaths, MemoryConfig},
    core::memory::{compact_session, turn_count},
};

pub fn maybe_compact(
    paths: &AssistantPaths,
    store: &SqliteStore,
    session_id: &str,
    config: &MemoryConfig,
) -> Result<Option<String>, String> {
    let turn_count = turn_count(store, session_id)?;
    if turn_count < config.compact_after_turns {
        return Ok(None);
    }
    Ok(Some(compact_session(
        paths,
        store,
        session_id,
        config.retain_recent_turns,
    )?))
}
