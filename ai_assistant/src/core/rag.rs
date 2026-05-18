use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    adapters::storage::SqliteStore,
    util::{now_epoch, path_title, read_to_string, sql_escape, truncate},
};

const SUPPORTED_EXTENSIONS: &[&str] = &["md", "txt", "json"];

pub fn index_path(store: &SqliteStore, path: &Path) -> Result<usize, String> {
    let files = collect_files(path)?;
    let mut indexed = 0;
    for file in files {
        let content = read_to_string(&file)?;
        let title = extract_title(&content).unwrap_or_else(|| path_title(&file));
        let path_string = file.to_string_lossy().to_string();
        let kind = file
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("txt")
            .to_string();
        store.exec(&format!(
            "DELETE FROM rag_documents WHERE path = '{}'; DELETE FROM rag_fts WHERE path = '{}';",
            sql_escape(&path_string),
            sql_escape(&path_string)
        ))?;
        store.exec(&format!(
            "INSERT INTO rag_documents (path, kind, title, content, indexed_at) VALUES ('{}', '{}', '{}', '{}', {});
             INSERT INTO rag_fts (path, title, content) VALUES ('{}', '{}', '{}');",
            sql_escape(&path_string),
            sql_escape(&kind),
            sql_escape(&title),
            sql_escape(&content),
            now_epoch(),
            sql_escape(&path_string),
            sql_escape(&title),
            sql_escape(&content)
        ))?;
        indexed += 1;
    }
    Ok(indexed)
}

pub fn search(store: &SqliteStore, query: &str, limit: usize) -> Result<Vec<String>, String> {
    let escaped = sql_escape(query);
    match store.query(&format!(
        "SELECT path, title, snippet(rag_fts, 2, '[', ']', '...', 12) FROM rag_fts WHERE rag_fts MATCH '{}' LIMIT {};",
        escaped, limit
    )) {
        Ok(rows) if !rows.is_empty() => Ok(rows
            .into_iter()
            .filter_map(|row| {
                if row.len() < 3 {
                    None
                } else {
                    Some(format!("{} :: {} :: {}", row[0], row[1], row[2]))
                }
            })
            .collect()),
        _ => {
            let rows = store.query(&format!(
                "SELECT path, title, content FROM rag_documents WHERE title LIKE '%{escaped}%' OR content LIKE '%{escaped}%' ORDER BY indexed_at DESC LIMIT {limit};"
            ))?;
            Ok(rows
                .into_iter()
                .filter_map(|row| {
                    if row.len() < 3 {
                        None
                    } else {
                        Some(format!("{} :: {} :: {}", row[0], row[1], truncate(&row[2], 120)))
                    }
                })
                .collect())
        }
    }
}

fn collect_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        if supported(path) {
            return Ok(vec![path.to_path_buf()]);
        }
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in
        fs::read_dir(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            files.extend(collect_files(&entry_path)?);
        } else if supported(&entry_path) {
            files.push(entry_path);
        }
    }
    Ok(files)
}

fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

fn extract_title(contents: &str) -> Option<String> {
    contents
        .lines()
        .find(|line| line.starts_with("# "))
        .map(|line| line.trim_start_matches("# ").trim().to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{adapters::storage::SqliteStore, util::unique_temp_dir};

    use super::{index_path, search};

    #[test]
    fn rag_indexes_and_searches_markdown_documents() {
        let root = unique_temp_dir("assistant-rag-unit");
        let source = root.join("notes");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("alpha.md"),
            "# Alpha\n\nA note about raspberry pi memory compaction.",
        )
        .unwrap();
        fs::write(
            source.join("beta.txt"),
            "scheduler reminder for offline inference",
        )
        .unwrap();

        let store = SqliteStore::from_path(root.join("assistant.db")).unwrap();
        let indexed = index_path(&store, &source).unwrap();
        assert_eq!(indexed, 2);

        let results = search(&store, "memory", 5).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("Alpha"));
    }
}
