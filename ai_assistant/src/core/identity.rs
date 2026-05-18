use crate::{
    config::{AssistantPaths, IdentityConfig, resolve_profile_path},
    util::read_to_string,
};
use std::fs;

#[derive(Clone, Debug)]
pub struct IdentityProfile {
    pub name: String,
    pub style: String,
    pub system_instruction: String,
    pub markdown_profile: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserProfile {
    pub name: String,
    pub telegram_handle: String,
    pub role: String,
    pub about: String,
    pub goals: String,
    pub preferences: String,
}

impl IdentityProfile {
    pub fn load(paths: &AssistantPaths, config: &IdentityConfig) -> Result<Self, String> {
        let markdown_profile = read_to_string(&resolve_profile_path(paths))?;
        Ok(Self {
            name: config.name.clone(),
            style: config.style.clone(),
            system_instruction: config.system_instruction.clone(),
            markdown_profile,
        })
    }

    pub fn known_user_facts(&self) -> Vec<String> {
        extract_user_profile_facts(&self.markdown_profile)
    }
}

pub fn write_assistant_profile(
    paths: &AssistantPaths,
    identity: &IdentityConfig,
    user: &UserProfile,
) -> Result<(), String> {
    let path = resolve_profile_path(paths);
    fs::write(&path, render_assistant_profile(identity, user))
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

pub fn render_assistant_profile(identity: &IdentityConfig, user: &UserProfile) -> String {
    let mut lines = vec![
        "# Assistant Profile".to_string(),
        String::new(),
        format!("Name: {}", identity.name),
        String::new(),
        "Purpose:".to_string(),
        "- Serve as a local-first Raspberry Pi assistant.".to_string(),
        "- Preserve user context without cloud dependencies.".to_string(),
        "- Prefer deterministic formatting and low-resource execution.".to_string(),
        String::new(),
        "Communication:".to_string(),
        format!("- {}", identity.style.trim()),
        "- Explain degraded states clearly.".to_string(),
        "- Keep outputs practical for terminal use.".to_string(),
        String::new(),
        "## User Profile".to_string(),
        format!("Name: {}", user.name.trim()),
    ];
    if !user.telegram_handle.trim().is_empty() {
        lines.push(format!("Telegram: {}", user.telegram_handle.trim()));
    }
    if !user.role.trim().is_empty() {
        lines.push(format!("Role: {}", user.role.trim()));
    }
    if !user.about.trim().is_empty() {
        lines.push("About:".to_string());
        lines.push(format!("- {}", user.about.trim()));
    }
    if !user.goals.trim().is_empty() {
        lines.push("Current goals:".to_string());
        lines.push(format!("- {}", user.goals.trim()));
    }
    if !user.preferences.trim().is_empty() {
        lines.push("Preferences:".to_string());
        lines.push(format!("- {}", user.preferences.trim()));
    }
    lines.join("\n") + "\n"
}

pub fn extract_user_profile_facts(profile: &str) -> Vec<String> {
    let mut in_user_section = false;
    let mut facts = Vec::new();

    for raw_line in profile.lines() {
        let line = raw_line.trim();
        if line.starts_with("## ") {
            in_user_section = line.eq_ignore_ascii_case("## User Profile");
            continue;
        }
        if !in_user_section || line.is_empty() {
            continue;
        }
        if line.ends_with(':') {
            continue;
        }
        facts.push(line.trim_start_matches("- ").to_string());
        if facts.len() >= 6 {
            break;
        }
    }

    facts
}

#[cfg(test)]
mod tests {
    use crate::config::IdentityConfig;

    use super::{UserProfile, extract_user_profile_facts, render_assistant_profile};

    #[test]
    fn extracts_user_profile_facts_from_markdown_section() {
        let facts = extract_user_profile_facts(
            "# Assistant Profile

Name: Kumo

Purpose:
- Stay local

## User Profile
Name: David
Telegram: @dbong
Role: Builder
About:
- Builds local AI systems
Preferences:
- Likes direct answers
",
        );

        assert_eq!(
            facts,
            vec![
                "Name: David",
                "Telegram: @dbong",
                "Role: Builder",
                "Builds local AI systems",
                "Likes direct answers",
            ]
        );
    }

    #[test]
    fn renders_assistant_profile_with_user_section() {
        let rendered = render_assistant_profile(
            &IdentityConfig {
                name: "Ayaka".into(),
                style: "direct and concise".into(),
                system_instruction: "Stay local".into(),
            },
            &UserProfile {
                name: "HardCoder".into(),
                telegram_handle: "@davidb2021".into(),
                role: "Builder".into(),
                about: "Builds local AI systems".into(),
                goals: "Improve the bot".into(),
                preferences: "short practical replies".into(),
            },
        );

        assert!(rendered.contains("Name: Ayaka"));
        assert!(rendered.contains("## User Profile"));
        assert!(rendered.contains("Name: HardCoder"));
        assert!(rendered.contains("Telegram: @davidb2021"));
    }
}
