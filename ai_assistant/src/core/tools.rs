use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use crate::config::{AssistantPaths, ToolConfig, write_tool_config};

#[derive(Clone, Debug)]
pub struct ToolExecutor {
    allowlist: Vec<String>,
    root: PathBuf,
}

impl ToolExecutor {
    pub fn new(allowlist: Vec<String>, root: PathBuf) -> Self {
        Self { allowlist, root }
    }

    pub fn allowlist(&self) -> &[String] {
        &self.allowlist
    }

    pub fn run(&self, command: &str, args: &[String]) -> Result<String, String> {
        if !self.allowlist.iter().any(|allowed| allowed == command) {
            return Err(format!("command `{command}` is not allowlisted"));
        }
        let output = Command::new(command)
            .args(args)
            .output()
            .map_err(|error| format!("failed to execute `{command}`: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "command `{command}` failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn read_file(&self, relative_path: &str) -> Result<String, String> {
        let path = self.resolve_relative(relative_path)?;
        fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))
    }

    pub fn write_markdown(&self, relative_path: &str, contents: &str) -> Result<String, String> {
        let path = self.resolve_relative(relative_path)?;
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension != "md" {
            return Err("markdown editing only supports .md files".to_string());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::write(&path, contents)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        Ok(format!("wrote {}", path.display()))
    }

    pub fn append_markdown(&self, relative_path: &str, contents: &str) -> Result<String, String> {
        let path = self.resolve_relative(relative_path)?;
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension != "md" {
            return Err("markdown editing only supports .md files".to_string());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        let existing = if path.exists() {
            fs::read_to_string(&path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?
        } else {
            String::new()
        };
        fs::write(&path, format!("{existing}{contents}"))
            .map_err(|error| format!("failed to append {}: {error}", path.display()))?;
        Ok(format!("appended {}", path.display()))
    }

    fn resolve_relative(&self, relative_path: &str) -> Result<PathBuf, String> {
        let candidate = Path::new(relative_path);
        if candidate.is_absolute() {
            return Err("absolute paths are not allowed".to_string());
        }
        if candidate.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err("path traversal is not allowed".to_string());
        }
        Ok(self.root.join(candidate))
    }
}

pub fn add_tool(
    paths: &AssistantPaths,
    config: &ToolConfig,
    command: &str,
) -> Result<String, String> {
    validate_command_name(command)?;
    let mut next = config.allowlist.clone();
    if !next.iter().any(|item| item == command) {
        next.push(command.to_string());
        next.sort();
    }
    write_tool_config(paths, &ToolConfig { allowlist: next })?;
    Ok(format!("tool allowlisted: {command}"))
}

pub fn remove_tool(
    paths: &AssistantPaths,
    config: &ToolConfig,
    command: &str,
) -> Result<String, String> {
    let mut next = config.allowlist.clone();
    next.retain(|item| item != command);
    write_tool_config(paths, &ToolConfig { allowlist: next })?;
    Ok(format!("tool removed: {command}"))
}

fn validate_command_name(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("tool command cannot be empty".to_string());
    }
    if command.contains('/') || command.contains('\\') || command.contains('\0') {
        return Err("tool command must be a bare executable name".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::util::unique_temp_dir;

    use super::ToolExecutor;

    #[test]
    fn tool_executor_blocks_non_allowlisted_commands() {
        let executor = ToolExecutor::new(vec!["date".into()], unique_temp_dir("tool-allowlist"));
        let result = executor.run("echo", &["hello".into()]);
        assert!(result.is_err());
    }

    #[test]
    fn tool_executor_supports_markdown_read_and_write() {
        let root = unique_temp_dir("tool-markdown");
        let executor = ToolExecutor::new(vec!["date".into()], root.clone());

        executor
            .write_markdown("data/notes/test.md", "# Test\n\nhello")
            .unwrap();
        executor
            .append_markdown("data/notes/test.md", "\nworld")
            .unwrap();
        let content = executor.read_file("data/notes/test.md").unwrap();

        assert_eq!(content, "# Test\n\nhello\nworld");
        assert!(fs::metadata(root.join("data/notes/test.md")).is_ok());
    }
}
