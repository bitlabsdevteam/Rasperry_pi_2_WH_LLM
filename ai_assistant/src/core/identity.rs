use crate::{
    config::{AssistantPaths, IdentityConfig, resolve_profile_path},
    util::read_to_string,
};

#[derive(Clone, Debug)]
pub struct IdentityProfile {
    pub name: String,
    pub style: String,
    pub system_instruction: String,
    pub markdown_profile: String,
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
    use super::extract_user_profile_facts;

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
}
