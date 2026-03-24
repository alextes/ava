use std::path::Path;

use serde::Deserialize;

use crate::config;

/// a secret dependency declared in a skill's frontmatter.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillSecret {
    /// env var name to inject (e.g. GMAIL_CLIENT_ID)
    pub name: String,
    /// source URI (e.g. vault://gmail-personal-client-id)
    pub source: String,
}

/// a skill loaded from ~/.ava/skills/<name>/SKILL.md.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// whether users can invoke this skill with /skill-name in chat
    pub user_invocable: bool,
    /// when true, the skill is hidden from the system prompt and the model
    /// cannot activate it via tool call. only user invocation works.
    pub disable_model_invocation: bool,
    /// secrets this skill needs, resolved from vault at activation time
    pub secrets: Vec<SkillSecret>,
    /// the markdown body (everything after the YAML frontmatter)
    pub body: String,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    user_invocable: bool,
    #[serde(default)]
    disable_model_invocation: bool,
    #[serde(default)]
    secrets: Vec<SkillSecret>,
}

/// parse a SKILL.md file into a Skill.
/// expects YAML frontmatter delimited by `---` lines, followed by the body.
fn parse_skill(content: &str) -> Result<Skill, String> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return Err("missing YAML frontmatter (expected leading ---)".into());
    }

    let after_first = &content[3..];
    let end = after_first
        .find("\n---")
        .ok_or("missing closing --- for frontmatter")?;

    let yaml_str = &after_first[..end];
    let body_start = end + 4; // skip \n---
    let body = after_first[body_start..].trim().to_string();

    let fm: SkillFrontmatter =
        serde_yaml::from_str(yaml_str).map_err(|e| format!("invalid frontmatter: {e}"))?;

    Ok(Skill {
        name: fm.name,
        description: fm.description,
        user_invocable: fm.user_invocable,
        disable_model_invocation: fm.disable_model_invocation,
        secrets: fm.secrets,
        body,
    })
}

/// load all skills from ~/.ava/skills/.
/// each skill is a directory containing a SKILL.md file.
pub fn load_skills() -> Vec<Skill> {
    let skills_dir = config::ava_home_dir().join("skills");
    load_skills_from(&skills_dir)
}

fn load_skills_from(skills_dir: &Path) -> Vec<Skill> {
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut skills = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_file = path.join("SKILL.md");
        let content = match std::fs::read_to_string(&skill_file) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %skill_file.display(), %e, "failed to read skill file");
                continue;
            }
        };

        match parse_skill(&content) {
            Ok(skill) => {
                tracing::info!(name = %skill.name, "loaded skill");
                skills.push(skill);
            }
            Err(e) => {
                tracing::warn!(path = %skill_file.display(), %e, "failed to parse skill");
            }
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_basic() {
        let content = r#"---
name: summarize
description: summarize a document or conversation
user_invocable: true
---

please summarize the following content concisely.
"#;
        let skill = parse_skill(content).unwrap();
        assert_eq!(skill.name, "summarize");
        assert_eq!(skill.description, "summarize a document or conversation");
        assert!(skill.user_invocable);
        assert!(!skill.disable_model_invocation);
        assert_eq!(
            skill.body,
            "please summarize the following content concisely."
        );
    }

    #[test]
    fn test_parse_skill_defaults() {
        let content = r#"---
name: greet
description: say hello
---

hello!
"#;
        let skill = parse_skill(content).unwrap();
        assert!(!skill.user_invocable);
        assert!(!skill.disable_model_invocation);
    }

    #[test]
    fn test_parse_skill_disable_model() {
        let content = r#"---
name: internal
description: internal only
disable_model_invocation: true
user_invocable: true
---

this skill is user-only.
"#;
        let skill = parse_skill(content).unwrap();
        assert!(skill.user_invocable);
        assert!(skill.disable_model_invocation);
    }

    #[test]
    fn test_parse_skill_missing_frontmatter() {
        let content = "just some text without frontmatter";
        assert!(parse_skill(content).is_err());
    }

    #[test]
    fn test_parse_skill_missing_closing() {
        let content = "---\nname: broken\n";
        assert!(parse_skill(content).is_err());
    }

    #[test]
    fn test_parse_skill_invalid_yaml() {
        let content = "---\n: bad yaml [[\n---\nbody";
        assert!(parse_skill(content).is_err());
    }

    #[test]
    fn test_load_skills_from_dir() {
        let dir = tempfile::tempdir().unwrap();

        // create a valid skill
        let skill_dir = dir.path().join("summarize");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: summarize\ndescription: summarize things\n---\ndo the summary",
        )
        .unwrap();

        // create another skill
        let skill_dir2 = dir.path().join("translate");
        std::fs::create_dir(&skill_dir2).unwrap();
        std::fs::write(
            skill_dir2.join("SKILL.md"),
            "---\nname: translate\ndescription: translate text\nuser_invocable: true\n---\ntranslate this",
        )
        .unwrap();

        // create an invalid one (no SKILL.md)
        let bad_dir = dir.path().join("empty");
        std::fs::create_dir(&bad_dir).unwrap();

        let skills = load_skills_from(dir.path());
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "summarize");
        assert_eq!(skills[1].name, "translate");
        assert!(skills[1].user_invocable);
    }

    #[test]
    fn test_load_skills_nonexistent_dir() {
        let skills = load_skills_from(Path::new("/nonexistent/path"));
        assert!(skills.is_empty());
    }
}
