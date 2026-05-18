use std::{thread, time::Duration};

use crate::{
    adapters::storage::SqliteStore,
    config::{AssistantPaths, SchedulerConfig},
    core::{
        memory::{cleanup_expired_memories, summarize_session},
        rag,
        tasks::{add_task, update_task},
        tools::ToolExecutor,
    },
    util::{now_epoch, sql_escape},
};

#[derive(Clone, Debug)]
pub struct Job {
    pub id: i64,
    pub name: String,
    pub every_minutes: i64,
    pub action: String,
    pub enabled: bool,
}

pub fn add_job(
    store: &SqliteStore,
    name: &str,
    every_minutes: i64,
    action: &str,
) -> Result<String, String> {
    store.exec(&format!(
        "INSERT OR REPLACE INTO jobs (name, every_minutes, action, enabled, created_at) VALUES ('{}', {}, '{}', 1, {});",
        sql_escape(name),
        every_minutes,
        sql_escape(action),
        now_epoch()
    ))?;
    Ok(format!(
        "scheduled job `{name}` every {every_minutes} minutes"
    ))
}

pub fn list_jobs(store: &SqliteStore) -> Result<Vec<Job>, String> {
    let rows = store
        .query("SELECT id, name, every_minutes, action, enabled FROM jobs ORDER BY id ASC;")?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            if row.len() < 5 {
                return None;
            }
            Some(Job {
                id: row[0].parse().ok()?,
                name: row[1].clone(),
                every_minutes: row[2].parse().ok()?,
                action: row[3].clone(),
                enabled: row[4] == "1",
            })
        })
        .collect())
}

pub fn run_due_jobs(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &SchedulerConfig,
    tools: &ToolExecutor,
) -> Result<Vec<String>, String> {
    let now = now_epoch();
    let rows = store.query(&format!(
        "SELECT id, name, every_minutes, action, enabled FROM jobs WHERE enabled = 1 AND (last_run IS NULL OR last_run + (every_minutes * 60) <= {}) ORDER BY id ASC LIMIT {};",
        now,
        config.max_jobs_per_tick
    ))?;

    let mut logs = Vec::new();
    for row in rows {
        if row.len() < 5 {
            continue;
        }
        let id: i64 = row[0].parse().unwrap_or_default();
        let name = row[1].clone();
        let action = row[3].clone();
        let result = execute_action(paths, store, tools, &action, config.allow_shell_jobs)?;
        store.exec(&format!(
            "UPDATE jobs SET last_run = {} WHERE id = {};",
            now, id
        ))?;
        logs.push(format!("{name}: {result}"));
    }
    Ok(logs)
}

pub fn serve_forever(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &SchedulerConfig,
    tools: &ToolExecutor,
    iterations: Option<usize>,
) -> Result<Vec<String>, String> {
    let mut output = Vec::new();
    let mut completed = 0usize;
    loop {
        output.extend(run_due_jobs(paths, store, config, tools)?);
        completed += 1;
        if iterations.is_some_and(|value| completed >= value) {
            break;
        }
        thread::sleep(Duration::from_secs(config.poll_seconds as u64));
    }
    Ok(output)
}

fn execute_action(
    paths: &AssistantPaths,
    store: &SqliteStore,
    tools: &ToolExecutor,
    action: &str,
    allow_shell_jobs: bool,
) -> Result<String, String> {
    if action == "summarize" {
        return summarize_session(paths, store, "default");
    }
    if let Some(path) = action.strip_prefix("rag index ") {
        let count = rag::index_path(store, std::path::Path::new(path.trim()))?;
        return Ok(format!("indexed {count} files"));
    }
    if action == "cleanup memories" {
        let count = cleanup_expired_memories(store)?;
        return Ok(format!("expired memories removed: {count}"));
    }
    if let Some(title) = action.strip_prefix("task add ") {
        return add_task(store, title.trim(), 1);
    }
    if let Some(payload) = action.strip_prefix("task prioritize ") {
        let pieces = payload.splitn(2, ' ').collect::<Vec<_>>();
        if pieces.len() != 2 {
            return Err("task prioritize requires `<id> <priority>`".to_string());
        }
        let id = pieces[0]
            .parse::<i64>()
            .map_err(|_| "task id must be numeric".to_string())?;
        let priority = pieces[1]
            .trim()
            .parse::<i64>()
            .map_err(|_| "priority must be numeric".to_string())?;
        return update_task(store, id, None, None, Some(priority), None);
    }
    if let Some(shell) = action.strip_prefix("shell ") {
        if !allow_shell_jobs {
            return Err("shell jobs are disabled by config".to_string());
        }
        let pieces = shell
            .split_whitespace()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if pieces.is_empty() {
            return Err("shell job command is empty".to_string());
        }
        let command = pieces[0].clone();
        let args = pieces[1..].to_vec();
        return tools.run(&command, &args);
    }
    Ok(format!("recorded action `{action}`"))
}

#[cfg(test)]
mod tests {
    use crate::{
        adapters::storage::SqliteStore,
        config::{AssistantPaths, SchedulerConfig},
        core::tools::ToolExecutor,
        util::unique_temp_dir,
    };

    use super::{add_job, list_jobs, run_due_jobs};

    #[test]
    fn scheduler_runs_due_jobs() {
        let root = unique_temp_dir("assistant-scheduler-test");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let config = SchedulerConfig {
            poll_seconds: 1,
            max_jobs_per_tick: 4,
            allow_shell_jobs: false,
        };
        let tools = ToolExecutor::new(vec!["echo".into()], paths.root.clone());

        add_job(&store, "summary", 0, "summarize").unwrap();
        let jobs = list_jobs(&store).unwrap();
        assert_eq!(jobs.len(), 1);

        let logs = run_due_jobs(&paths, &store, &config, &tools).unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("summary"));
    }
}
