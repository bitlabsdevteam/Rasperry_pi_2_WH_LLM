use crate::{
    core::{
        memory::{MemoryRecord, Message},
        tasks::Task,
    },
    util::{token_estimate, truncate},
};

#[derive(Clone, Debug)]
pub struct HarnessInput {
    pub identity_name: String,
    pub identity_style: String,
    pub identity_profile: String,
    pub system_instruction: String,
    pub prefer_code_output: bool,
    pub user_intent: String,
    pub context_snippets: Vec<String>,
    pub personal_memories: Vec<MemoryRecord>,
    pub knowledge_memories: Vec<MemoryRecord>,
    pub runtime_memories: Vec<MemoryRecord>,
    pub tool_context: Vec<String>,
    pub skill_context: Vec<String>,
    pub tasks: Vec<Task>,
    pub safety_rules: Vec<String>,
    pub recent_messages: Vec<Message>,
    pub token_budget: usize,
}

pub fn build_prompt(input: &HarnessInput) -> String {
    let mut prompt = vec![
        format!("Assistant name: {}.", input.identity_name.trim()),
        format!(
            "Reply to the user directly. Do not address {} as if they were the user.",
            input.identity_name.trim()
        ),
        format!("Style: {}.", input.identity_style.trim()),
        truncate_line(input.system_instruction.trim(), 160),
        "Answer the user's latest message directly.".to_string(),
        "Do not repeat the prompt, identity profile, memory dump, task list, or internal notes."
            .to_string(),
    ];
    if input.prefer_code_output {
        prompt.push(
            "The user is asking for code. Return runnable code with correct indentation."
                .to_string(),
        );
        prompt.push(
            "Use a fenced code block when showing code. Keep any explanation brief and after the code."
                .to_string(),
        );
    } else {
        prompt.push("Keep the reply short and useful for Telegram.".to_string());
    }

    let identity_notes = compact_identity_notes(&input.identity_profile);
    if !identity_notes.is_empty() {
        prompt.push(format!("Identity notes: {}", identity_notes.join(" | ")));
    }
    let known_user = build_known_user_section(&input.identity_profile);
    if !known_user.is_empty() {
        prompt.push(format!("Known user:\n{known_user}"));
    }

    let base = prompt.join("\n");
    let available = input
        .token_budget
        .saturating_sub(token_estimate(&base))
        .max(64);

    let context = build_context_section(input, available / 2);
    if !context.is_empty() {
        prompt.push(format!("Recent context:\n{context}"));
    }

    let personal = build_memory_section(&input.personal_memories, available / 6);
    if !personal.is_empty() {
        prompt.push(format!("Personal memory:\n{personal}"));
    }

    let knowledge = build_memory_section(&input.knowledge_memories, available / 6);
    if !knowledge.is_empty() {
        prompt.push(format!("Knowledge memory:\n{knowledge}"));
    }

    let runtime = build_memory_section(&input.runtime_memories, available / 8);
    if !runtime.is_empty() {
        prompt.push(format!("Runtime state:\n{runtime}"));
    }

    let tasks = build_task_section(input, available / 6);
    if !tasks.is_empty() {
        prompt.push(format!("Pending tasks:\n{tasks}"));
    }

    let tools = build_tools_section(input, available / 8);
    if !tools.is_empty() {
        prompt.push(format!("Available tools:\n{tools}"));
    }

    let skills = build_skills_section(input, available / 6);
    if !skills.is_empty() {
        prompt.push(format!("Available skills:\n{skills}"));
    }

    let safety = build_safety_section(input);
    if !safety.is_empty() {
        prompt.push(format!("Safety rules:\n{safety}"));
    }

    prompt.join("\n\n")
}

fn build_context_section(input: &HarnessInput, budget: usize) -> String {
    let mut lines = Vec::new();
    for snippet in &input.context_snippets {
        let line = truncate_line(snippet, 72);
        if token_estimate(&lines.join("\n")) + token_estimate(&line) > budget / 3 {
            break;
        }
        lines.push(format!("- {line}"));
    }

    let mut recent = input.recent_messages.clone();
    recent.reverse();
    for message in recent {
        let line = format!(
            "- {}: {}",
            message.role,
            truncate_line(&message.content, 96)
        );
        if token_estimate(&lines.join("\n")) + token_estimate(&line) > budget {
            break;
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn build_memory_section(memories: &[MemoryRecord], budget: usize) -> String {
    let mut lines = Vec::new();
    for memory in memories {
        let line = format!(
            "- [{}] {} :: {}",
            memory.tags,
            truncate_line(&memory.title, 48),
            truncate_line(&memory.body, 72)
        );
        if token_estimate(&lines.join("\n")) + token_estimate(&line) > budget.max(24) {
            break;
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn build_tools_section(input: &HarnessInput, budget: usize) -> String {
    let mut lines = Vec::new();
    for tool in &input.tool_context {
        let line = format!("- {}", truncate_line(tool, 36));
        if token_estimate(&lines.join("\n")) + token_estimate(&line) > budget.max(16) {
            break;
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn build_skills_section(input: &HarnessInput, budget: usize) -> String {
    let mut lines = Vec::new();
    for skill in &input.skill_context {
        let line = format!("- {}", truncate_line(skill, 120));
        if token_estimate(&lines.join("\n")) + token_estimate(&line) > budget.max(24) {
            break;
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn build_task_section(input: &HarnessInput, budget: usize) -> String {
    let mut lines = Vec::new();
    for task in input.tasks.iter().take(3) {
        let line = format!(
            "- #{} [{}] {}",
            task.id,
            task.status,
            truncate_line(&task.title, 40)
        );
        if token_estimate(&lines.join("\n")) + token_estimate(&line) > budget.max(16) {
            break;
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn build_safety_section(input: &HarnessInput) -> String {
    let lines = if input.safety_rules.is_empty() {
        vec!["- Stay offline-first and deterministic.".to_string()]
    } else {
        input
            .safety_rules
            .iter()
            .take(3)
            .map(|rule| format!("- {}", truncate_line(rule, 56)))
            .collect()
    };
    lines.join("\n")
}

fn compact_identity_notes(profile: &str) -> Vec<String> {
    let mut notes = Vec::new();
    for line in profile.lines().map(str::trim) {
        if line.eq_ignore_ascii_case("## User Profile") {
            break;
        }
        if line.is_empty()
            || line.starts_with('#')
            || line.ends_with(':')
            || line.starts_with("Name:")
        {
            continue;
        }
        notes.push(line.trim_start_matches("- ").to_string());
        if notes.len() >= 4 {
            break;
        }
    }
    notes
}

fn build_known_user_section(profile: &str) -> String {
    let mut in_user_section = false;
    let mut lines = Vec::new();

    for raw_line in profile.lines() {
        let line = raw_line.trim();
        if line.starts_with("## ") {
            in_user_section = line.eq_ignore_ascii_case("## User Profile");
            continue;
        }
        if !in_user_section || line.is_empty() || line.ends_with(':') {
            continue;
        }
        lines.push(format!(
            "- {}",
            truncate_line(line.trim_start_matches("- "), 96)
        ));
        if lines.len() >= 6 {
            break;
        }
    }

    lines.join("\n")
}

fn truncate_line(value: &str, max: usize) -> String {
    truncate(&value.replace('\n', " ").replace('\r', " "), max)
}

#[cfg(test)]
mod tests {
    use super::{HarnessInput, build_prompt};
    use crate::core::{memory::Message, tasks::Task};

    #[test]
    fn prompt_builder_keeps_required_layer_order() {
        let prompt = build_prompt(&HarnessInput {
            identity_name: "Kumo".into(),
            identity_style: "direct".into(),
            identity_profile: "Profile".into(),
            system_instruction: "Follow policy".into(),
            prefer_code_output: false,
            user_intent: "Plan my day".into(),
            context_snippets: vec!["today is sunny".into()],
            personal_memories: vec![],
            knowledge_memories: vec![],
            runtime_memories: vec![],
            tool_context: vec!["date".into()],
            skill_context: vec!["daily-notes: append operational notes".into()],
            tasks: vec![Task::new_for_test(1, "Check logs", "pending", 1)],
            safety_rules: vec!["Never call cloud APIs.".into()],
            recent_messages: vec![Message::new("user", "hello there")],
            token_budget: 128,
        });

        assert!(prompt.contains("Assistant name: Kumo."));
        assert!(!prompt.contains("User: Plan my day"));
        assert!(!prompt.ends_with("Assistant:"));
        assert!(prompt.contains("Pending tasks:"));
        assert!(prompt.contains("Available skills:"));
    }

    #[test]
    fn prompt_builder_prunes_to_token_budget() {
        let prompt = build_prompt(&HarnessInput {
            identity_name: "Kumo".into(),
            identity_style: "direct".into(),
            identity_profile: "Profile".into(),
            system_instruction: "Follow policy".into(),
            prefer_code_output: false,
            user_intent: "Summarize".into(),
            context_snippets: vec!["x".repeat(400)],
            personal_memories: vec![],
            knowledge_memories: vec![],
            runtime_memories: vec![],
            tool_context: vec![],
            skill_context: vec![],
            tasks: vec![],
            safety_rules: vec!["Stay local.".into()],
            recent_messages: vec![Message::new("user", &"y ".repeat(200))],
            token_budget: 64,
        });

        assert!(prompt.split_whitespace().count() <= 140);
    }

    #[test]
    fn prompt_builder_avoids_markdown_scaffold_headers() {
        let prompt = build_prompt(&HarnessInput {
            identity_name: "Kumo".into(),
            identity_style: "direct".into(),
            identity_profile: "# Assistant Profile\n\nName: Kumo\n\nPurpose:\n- Stay local".into(),
            system_instruction: "Follow policy".into(),
            prefer_code_output: false,
            user_intent: "hello".into(),
            context_snippets: vec![],
            personal_memories: vec![],
            knowledge_memories: vec![],
            runtime_memories: vec![],
            tool_context: vec![],
            skill_context: vec![],
            tasks: vec![],
            safety_rules: vec![],
            recent_messages: vec![],
            token_budget: 96,
        });

        assert!(!prompt.contains("## Identity Layer"));
        assert!(!prompt.contains("# Assistant Profile"));
        assert!(!prompt.contains("Assistant:"));
    }

    #[test]
    fn prompt_builder_includes_known_user_section() {
        let prompt = build_prompt(&HarnessInput {
            identity_name: "Kumo".into(),
            identity_style: "direct".into(),
            identity_profile: "# Assistant Profile

Name: Kumo

Purpose:
- Stay local

## User Profile
Name: David
Telegram: @dbong
Role: Builder
Preferences:
- Prefers concise answers"
                .into(),
            system_instruction: "Follow policy".into(),
            prefer_code_output: false,
            user_intent: "Who am I?".into(),
            context_snippets: vec![],
            personal_memories: vec![],
            knowledge_memories: vec![],
            runtime_memories: vec![],
            tool_context: vec![],
            skill_context: vec![],
            tasks: vec![],
            safety_rules: vec![],
            recent_messages: vec![],
            token_budget: 128,
        });

        assert!(prompt.contains("Known user:"));
        assert!(prompt.contains("Name: David"));
        assert!(prompt.contains("Telegram: @dbong"));
    }

    #[test]
    fn prompt_builder_switches_to_code_mode() {
        let prompt = build_prompt(&HarnessInput {
            identity_name: "Kumo".into(),
            identity_style: "direct".into(),
            identity_profile: "Profile".into(),
            system_instruction: "Follow policy".into(),
            prefer_code_output: true,
            user_intent: "Write python code with a for loop".into(),
            context_snippets: vec![],
            personal_memories: vec![],
            knowledge_memories: vec![],
            runtime_memories: vec![],
            tool_context: vec![],
            skill_context: vec![],
            tasks: vec![],
            safety_rules: vec![],
            recent_messages: vec![],
            token_budget: 128,
        });

        assert!(prompt.contains("The user is asking for code."));
        assert!(prompt.contains("Use a fenced code block"));
        assert!(!prompt.contains("Keep the reply short and useful for Telegram."));
    }
}
