use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::{
    config::AssistantPaths,
    core::tools::ToolExecutor,
    util::{ensure_dir, path_title, truncate},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub tools: Vec<String>,
    pub instructions: String,
    pub steps: Vec<SkillStep>,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillStep {
    Command { command: String, args: Vec<String> },
    Read { path: String },
    WriteMarkdown { path: String, contents: String },
    AppendMarkdown { path: String, contents: String },
}

pub fn create_skill(
    paths: &AssistantPaths,
    name: &str,
    description: &str,
    triggers: &[String],
    tools: &[String],
    instructions: &str,
    steps: &[String],
) -> Result<String, String> {
    ensure_dir(&paths.skills_dir)?;
    let normalized = normalize_skill_name(name)?;
    let parsed_steps = parse_step_lines(steps)?;
    let path = paths.skills_dir.join(format!("{normalized}.md"));
    fs::write(
        &path,
        render_skill_markdown(
            &normalized,
            description,
            triggers,
            tools,
            instructions,
            &parsed_steps,
        ),
    )
    .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(format!("skill installed: {normalized}"))
}

pub fn install_skill_file(paths: &AssistantPaths, source: &Path) -> Result<String, String> {
    ensure_dir(&paths.skills_dir)?;
    let contents = fs::read_to_string(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
    let mut skill = parse_skill_markdown(source.to_path_buf(), &contents)?;
    if skill.name.trim().is_empty() {
        skill.name = normalize_skill_name(&path_title(source))?;
    }
    let normalized = normalize_skill_name(&skill.name)?;
    let destination = paths.skills_dir.join(format!("{normalized}.md"));
    fs::write(&destination, contents)
        .map_err(|error| format!("failed to write {}: {error}", destination.display()))?;
    Ok(format!("skill installed: {normalized}"))
}

pub fn list_skills(paths: &AssistantPaths) -> Result<Vec<Skill>, String> {
    ensure_dir(&paths.skills_dir)?;
    let mut skills = Vec::new();
    for entry in fs::read_dir(&paths.skills_dir)
        .map_err(|error| format!("failed to read {}: {error}", paths.skills_dir.display()))?
    {
        let entry = entry.map_err(|error| format!("failed to read skill entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        skills.push(parse_skill_markdown(path, &contents)?);
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
}

pub fn find_skill(paths: &AssistantPaths, name: &str) -> Result<Skill, String> {
    let normalized = normalize_skill_name(name)?;
    list_skills(paths)?
        .into_iter()
        .find(|skill| normalize_skill_name(&skill.name).ok().as_deref() == Some(&normalized))
        .ok_or_else(|| format!("skill `{name}` not found"))
}

pub fn select_skills(
    paths: &AssistantPaths,
    task: &str,
    limit: usize,
) -> Result<Vec<Skill>, String> {
    let task_lower = task.to_ascii_lowercase();
    let mut selected = Vec::new();
    for skill in list_skills(paths)? {
        let name_match =
            !skill.name.is_empty() && task_lower.contains(&skill.name.to_ascii_lowercase());
        let trigger_match = skill.triggers.iter().any(|trigger| {
            let trigger = trigger.trim().to_ascii_lowercase();
            !trigger.is_empty() && task_lower.contains(&trigger)
        });
        if name_match || trigger_match {
            selected.push(skill);
        }
        if selected.len() >= limit {
            break;
        }
    }
    Ok(selected)
}

pub fn run_skill(
    paths: &AssistantPaths,
    tools: &ToolExecutor,
    selector: &str,
    task: &str,
) -> Result<String, String> {
    let skill = if selector == "auto" {
        select_skills(paths, task, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| "no skill matched the task".to_string())?
    } else {
        find_skill(paths, selector)?
    };

    let mut lines = vec![
        format!("skill: {}", skill.name),
        format!("description: {}", skill.description),
        format!("task: {task}"),
    ];
    if !skill.instructions.trim().is_empty() {
        lines.push(format!(
            "instructions: {}",
            truncate(&skill.instructions.replace('\n', " "), 220)
        ));
    }

    if skill.steps.is_empty() {
        lines.push("steps: none".to_string());
        return Ok(lines.join("\n"));
    }

    lines.push("step results:".to_string());
    for step in skill.steps {
        let rendered = render_step(step, task);
        let result = execute_step(tools, &rendered)?;
        lines.push(format!("- {result}"));
    }
    Ok(lines.join("\n"))
}

pub fn skill_prompt_context(skills: &[Skill]) -> Vec<String> {
    skills
        .iter()
        .map(|skill| {
            let tools = if skill.tools.is_empty() {
                "none".to_string()
            } else {
                skill.tools.join(", ")
            };
            format!(
                "{}: {} | triggers={} | tools={} | {}",
                skill.name,
                truncate(&skill.description, 80),
                skill.triggers.join(", "),
                tools,
                truncate(&skill.instructions.replace('\n', " "), 120)
            )
        })
        .collect()
}

fn execute_step(tools: &ToolExecutor, step: &SkillStep) -> Result<String, String> {
    match step {
        SkillStep::Command { command, args } => {
            let output = tools.run(command, args)?;
            Ok(format!("command `{command}` => {output}"))
        }
        SkillStep::Read { path } => {
            let content = tools.read_file(path)?;
            Ok(format!(
                "read `{path}` => {}",
                truncate(&content.replace('\n', " "), 160)
            ))
        }
        SkillStep::WriteMarkdown { path, contents } => tools.write_markdown(path, contents),
        SkillStep::AppendMarkdown { path, contents } => tools.append_markdown(path, contents),
    }
}

fn render_step(step: SkillStep, task: &str) -> SkillStep {
    match step {
        SkillStep::Command { command, args } => SkillStep::Command {
            command: render_template(&command, task),
            args: args
                .into_iter()
                .map(|arg| render_template(&arg, task))
                .collect(),
        },
        SkillStep::Read { path } => SkillStep::Read {
            path: render_template(&path, task),
        },
        SkillStep::WriteMarkdown { path, contents } => SkillStep::WriteMarkdown {
            path: render_template(&path, task),
            contents: render_template(&contents, task),
        },
        SkillStep::AppendMarkdown { path, contents } => SkillStep::AppendMarkdown {
            path: render_template(&path, task),
            contents: render_template(&contents, task),
        },
    }
}

fn render_template(value: &str, task: &str) -> String {
    value.replace("{{task}}", task)
}

fn parse_skill_markdown(path: PathBuf, contents: &str) -> Result<Skill, String> {
    let mut name = String::new();
    let mut description = String::new();
    let mut triggers = Vec::new();
    let mut tools = Vec::new();
    let mut instructions = Vec::new();
    let mut step_lines = Vec::new();
    let mut section = "";

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix("# Skill:") {
            name = value.trim().to_string();
            continue;
        }
        if line.eq_ignore_ascii_case("## Instructions") {
            section = "instructions";
            continue;
        }
        if line.eq_ignore_ascii_case("## Steps") {
            section = "steps";
            continue;
        }
        if let Some(value) = line.strip_prefix("Description:") {
            description = value.trim().to_string();
            continue;
        }
        if let Some(value) = line.strip_prefix("Triggers:") {
            triggers = split_csv(value);
            continue;
        }
        if let Some(value) = line.strip_prefix("Tools:") {
            tools = split_csv(value);
            continue;
        }
        if section == "instructions" {
            instructions.push(raw_line.to_string());
        } else if section == "steps" && !line.is_empty() {
            step_lines.push(line.trim_start_matches("- ").to_string());
        }
    }

    if name.trim().is_empty() {
        name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("skill")
            .to_string();
    }
    let name = normalize_skill_name(&name)?;
    let steps = parse_step_lines(&step_lines)?;
    Ok(Skill {
        name,
        description,
        triggers,
        tools,
        instructions: instructions.join("\n").trim().to_string(),
        steps,
        path,
    })
}

fn parse_step_lines(lines: &[String]) -> Result<Vec<SkillStep>, String> {
    let mut steps = Vec::new();
    for line in lines {
        let line = line.trim().trim_start_matches("- ").trim();
        if line.is_empty() {
            continue;
        }
        let Some((kind, payload)) = line.split_once(':') else {
            return Err(format!("invalid skill step `{line}`"));
        };
        match kind.trim().to_ascii_lowercase().as_str() {
            "command" => {
                let pieces = payload
                    .split_whitespace()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                if pieces.is_empty() {
                    return Err("command step requires a command".to_string());
                }
                steps.push(SkillStep::Command {
                    command: pieces[0].clone(),
                    args: pieces[1..].to_vec(),
                });
            }
            "read" => steps.push(SkillStep::Read {
                path: payload.trim().to_string(),
            }),
            "write_markdown" => {
                let (path, contents) = split_path_contents(payload, "write_markdown")?;
                steps.push(SkillStep::WriteMarkdown { path, contents });
            }
            "append_markdown" => {
                let (path, contents) = split_path_contents(payload, "append_markdown")?;
                steps.push(SkillStep::AppendMarkdown { path, contents });
            }
            other => return Err(format!("unsupported skill step `{other}`")),
        }
    }
    Ok(steps)
}

fn split_path_contents(payload: &str, kind: &str) -> Result<(String, String), String> {
    let (path, contents) = payload
        .split_once('|')
        .ok_or_else(|| format!("{kind} step requires `<path> | <contents>`"))?;
    Ok((path.trim().to_string(), contents.trim().to_string()))
}

fn render_skill_markdown(
    name: &str,
    description: &str,
    triggers: &[String],
    tools: &[String],
    instructions: &str,
    steps: &[SkillStep],
) -> String {
    let mut lines = vec![
        format!("# Skill: {name}"),
        String::new(),
        format!("Description: {description}"),
        format!("Triggers: {}", triggers.join(", ")),
        format!("Tools: {}", tools.join(", ")),
        String::new(),
        "## Instructions".to_string(),
        instructions.to_string(),
        String::new(),
        "## Steps".to_string(),
    ];
    if steps.is_empty() {
        lines.push(String::new());
    } else {
        lines.extend(steps.iter().map(format_step));
    }
    lines.join("\n")
}

fn format_step(step: &SkillStep) -> String {
    match step {
        SkillStep::Command { command, args } => {
            format!("- command: {} {}", command, args.join(" "))
                .trim()
                .to_string()
        }
        SkillStep::Read { path } => format!("- read: {path}"),
        SkillStep::WriteMarkdown { path, contents } => {
            format!("- write_markdown: {path} | {contents}")
        }
        SkillStep::AppendMarkdown { path, contents } => {
            format!("- append_markdown: {path} | {contents}")
        }
    }
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalize_skill_name(name: &str) -> Result<String, String> {
    let normalized = name
        .trim()
        .trim_end_matches(".md")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else if ch.is_whitespace() {
                '-'
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if normalized.is_empty() {
        return Err("skill name cannot be empty".to_string());
    }
    if Path::new(&normalized).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("invalid skill name".to_string());
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::{create_skill, list_skills, run_skill, select_skills};
    use crate::{config::AssistantPaths, core::tools::ToolExecutor, util::unique_temp_dir};

    #[test]
    fn skills_can_be_created_selected_and_run_with_tools() {
        let root = unique_temp_dir("assistant-skills");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        create_skill(
            &paths,
            "Daily Notes",
            "Capture a short operational note.",
            &["note".into()],
            &["echo".into()],
            "Append the task to the runtime note.",
            &[
                "append_markdown: data/notes/daily.md | {{task}}".into(),
                "command: echo done".into(),
            ],
        )
        .unwrap();

        let skills = list_skills(&paths).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(
            select_skills(&paths, "please make a note", 1)
                .unwrap()
                .len(),
            1
        );

        let tools = ToolExecutor::new(vec!["echo".into()], paths.root.clone());
        let output = run_skill(&paths, &tools, "auto", "write a note about check battery").unwrap();
        assert!(output.contains("skill: daily-notes"));
        assert!(output.contains("command `echo` => done"));
        assert_eq!(
            tools.read_file("data/notes/daily.md").unwrap(),
            "write a note about check battery"
        );
    }
}
