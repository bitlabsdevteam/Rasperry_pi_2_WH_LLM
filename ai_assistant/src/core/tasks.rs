use crate::{
    adapters::storage::SqliteStore,
    util::{now_epoch, sql_escape},
};

#[derive(Clone, Debug)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub priority: i64,
    pub notes: String,
}

impl Task {
    pub fn new_for_test(id: i64, title: &str, status: &str, priority: i64) -> Self {
        Self {
            id,
            title: title.to_string(),
            status: status.to_string(),
            priority,
            notes: String::new(),
        }
    }
}

pub fn add_task(store: &SqliteStore, title: &str, priority: i64) -> Result<String, String> {
    let now = now_epoch();
    store.exec(&format!(
        "INSERT INTO tasks (title, status, priority, created_at, updated_at) VALUES ('{}', 'pending', {}, {}, {});",
        sql_escape(title),
        priority,
        now,
        now
    ))?;
    Ok(format!("task added: {title}"))
}

pub fn list_tasks(store: &SqliteStore) -> Result<Vec<Task>, String> {
    let rows = store.query(
        "SELECT id, title, status, priority, notes FROM tasks ORDER BY status = 'done', priority DESC, id ASC;",
    )?;
    let tasks = rows
        .into_iter()
        .filter_map(|row| {
            if row.len() < 5 {
                return None;
            }
            Some(Task {
                id: row[0].parse().ok()?,
                title: row[1].clone(),
                status: row[2].clone(),
                priority: row[3].parse().ok()?,
                notes: row[4].clone(),
            })
        })
        .collect();
    Ok(tasks)
}

pub fn complete_task(store: &SqliteStore, id: i64) -> Result<String, String> {
    let now = now_epoch();
    store.exec(&format!(
        "UPDATE tasks SET status = 'done', updated_at = {} WHERE id = {};",
        now, id
    ))?;
    Ok(format!("task completed: {id}"))
}

pub fn update_task(
    store: &SqliteStore,
    id: i64,
    title: Option<&str>,
    status: Option<&str>,
    priority: Option<i64>,
    notes: Option<&str>,
) -> Result<String, String> {
    let existing = list_tasks(store)?
        .into_iter()
        .find(|task| task.id == id)
        .ok_or_else(|| format!("task not found: {id}"))?;

    let title = title.unwrap_or(&existing.title);
    let status = status.unwrap_or(&existing.status);
    let priority = priority.unwrap_or(existing.priority);
    let notes = notes.unwrap_or(&existing.notes);

    let now = now_epoch();
    store.exec(&format!(
        "UPDATE tasks SET title = '{}', status = '{}', priority = {}, notes = '{}', updated_at = {} WHERE id = {};",
        sql_escape(title),
        sql_escape(status),
        priority,
        sql_escape(notes),
        now,
        id
    ))?;
    Ok(format!("task updated: {id}"))
}

#[cfg(test)]
mod tests {
    use crate::{adapters::storage::SqliteStore, util::unique_temp_dir};

    use super::{add_task, complete_task, list_tasks, update_task};

    #[test]
    fn task_engine_adds_and_completes_tasks() {
        let root = unique_temp_dir("assistant-task-test");
        let store = SqliteStore::from_path(root.join("assistant.db")).unwrap();

        add_task(&store, "Inspect boot logs", 2).unwrap();
        add_task(&store, "Compact memory", 1).unwrap();
        let tasks = list_tasks(&store).unwrap();

        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].title, "Inspect boot logs");

        let id = tasks[0].id;
        complete_task(&store, id).unwrap();
        let refreshed = list_tasks(&store).unwrap();
        assert_eq!(
            refreshed.iter().find(|task| task.id == id).unwrap().status,
            "done"
        );
    }

    #[test]
    fn task_engine_updates_priority_and_notes() {
        let root = unique_temp_dir("assistant-task-update");
        let store = SqliteStore::from_path(root.join("assistant.db")).unwrap();

        add_task(&store, "Review sensors", 1).unwrap();
        let task = list_tasks(&store).unwrap().remove(0);

        update_task(
            &store,
            task.id,
            Some("Review morning sensors"),
            Some("in_progress"),
            Some(3),
            Some("start with CPU temp"),
        )
        .unwrap();

        let refreshed = list_tasks(&store).unwrap().remove(0);
        assert_eq!(refreshed.title, "Review morning sensors");
        assert_eq!(refreshed.status, "in_progress");
        assert_eq!(refreshed.priority, 3);
        assert_eq!(refreshed.notes, "start with CPU temp");
    }
}
