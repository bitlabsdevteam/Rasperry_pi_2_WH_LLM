use std::fs;

use ai_assistant::{adapters::storage::SqliteStore, core::rag, util::unique_temp_dir};

#[test]
fn rag_retrieval_returns_indexed_documents() {
    let root = unique_temp_dir("assistant-rag-integration");
    let docs = root.join("docs");
    fs::create_dir_all(&docs).unwrap();
    fs::write(
        docs.join("memory.md"),
        "# Memory\n\nContext compaction preserves important facts on constrained hardware.",
    )
    .unwrap();
    fs::write(
        docs.join("scheduler.txt"),
        "A scheduler runs maintenance jobs while the assistant stays offline.",
    )
    .unwrap();

    let store = SqliteStore::from_path(root.join("assistant.db")).unwrap();
    let indexed = rag::index_path(&store, &docs).unwrap();
    let results = rag::search(&store, "compaction", 5).unwrap();

    assert_eq!(indexed, 2);
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("Memory"));
}
