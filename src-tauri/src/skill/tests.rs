use super::*;

#[test]
fn test_parse_skill_basic() {
    let content = r#"---
name: test_skill
description: A test skill
trigger: /test
mode: ask
---

This is the template with $SELECTION."#;

    let skill = parse_skill(content).unwrap();
    assert_eq!(skill.name, "test_skill");
    assert_eq!(skill.description, "A test skill");
    assert_eq!(skill.trigger, "/test");
    assert_eq!(skill.mode, "ask");
    assert!(skill.agent.is_none());
    assert!(skill.tools.is_none());
    assert!(skill.template.contains("$SELECTION"));
}

#[test]
fn test_parse_skill_with_agent_and_tools() {
    let content = r#"---
name: review
description: Code review
trigger: /review
mode: agent
agent: reviewer
tools:
  - read_file
  - grep
---

Review this code."#;

    let skill = parse_skill(content).unwrap();
    assert_eq!(skill.agent, Some("reviewer".to_string()));
    assert_eq!(skill.tools, Some(vec!["read_file".to_string(), "grep".to_string()]));
    assert_eq!(skill.template, "Review this code.");
}

#[test]
fn test_parse_skill_default_mode() {
    let content = r#"---
name: simple
description: Simple skill
trigger: /simple
---

Do something."#;

    let skill = parse_skill(content).unwrap();
    assert_eq!(skill.mode, "ask");
}

#[test]
fn test_parse_skill_missing_frontmatter() {
    let result = parse_skill("No frontmatter here");
    assert!(result.is_err());
}

#[test]
fn test_parse_skill_missing_closing() {
    let content = "---\nname: broken\ntrigger: /broken\n";
    let result = parse_skill(content);
    assert!(result.is_err());
}

#[test]
fn test_template_render_all_vars() {
    let vars = SkillVars {
        selection: "let x = 1;".to_string(),
        file_path: "src/main.rs".to_string(),
        file_content: "fn main() {}".to_string(),
        project_path: "/home/user/project".to_string(),
        arguments: "focus on safety".to_string(),
        language: "rust".to_string(),
    };

    let template = "Lang: $LANGUAGE\nFile: $FILE_PATH\nProject: $PROJECT_PATH\nArgs: $ARGUMENTS\nCode: $SELECTION\nFull: $FILE_CONTENT";
    let rendered = vars.render(template);

    assert!(rendered.contains("Lang: rust"));
    assert!(rendered.contains("File: src/main.rs"));
    assert!(rendered.contains("Project: /home/user/project"));
    assert!(rendered.contains("Args: focus on safety"));
    assert!(rendered.contains("Code: let x = 1;"));
    assert!(rendered.contains("Full: fn main() {}"));
}

#[test]
fn test_template_render_no_vars() {
    let vars = SkillVars::default();
    let template = "No variables here";
    let rendered = vars.render(template);
    assert_eq!(rendered, "No variables here");
}

#[test]
fn test_template_render_empty_vars() {
    let vars = SkillVars {
        selection: "".to_string(),
        file_path: "".to_string(),
        ..Default::default()
    };
    let template = "Code: $SELECTION";
    let rendered = vars.render(template);
    assert_eq!(rendered, "Code: ");
}

#[test]
fn test_builtin_skills_parse() {
    for (filename, content) in builtin::builtin_skills() {
        let result = parse_skill(content);
        assert!(result.is_ok(), "Failed to parse built-in skill {}: {:?}", filename, result.err());
        let skill = result.unwrap();
        assert!(!skill.name.is_empty());
        assert!(!skill.trigger.is_empty());
        assert!(skill.trigger.starts_with('/'));
        assert!(!skill.template.is_empty());
    }
}

#[test]
fn test_builtin_review_skill() {
    let skill = parse_skill(builtin::REVIEW_SKILL).unwrap();
    assert_eq!(skill.trigger, "/review");
    assert_eq!(skill.mode, "agent");
    assert_eq!(skill.agent, Some("reviewer".to_string()));
}

#[test]
fn test_builtin_explain_skill() {
    let skill = parse_skill(builtin::EXPLAIN_SKILL).unwrap();
    assert_eq!(skill.trigger, "/explain");
    assert_eq!(skill.mode, "ask");
    assert!(skill.agent.is_none());
}

#[test]
fn test_builtin_refactor_skill() {
    let skill = parse_skill(builtin::REFACTOR_SKILL).unwrap();
    assert_eq!(skill.trigger, "/refactor");
    assert_eq!(skill.mode, "edit");
}

#[test]
fn test_builtin_tests_skill() {
    let skill = parse_skill(builtin::TESTS_SKILL).unwrap();
    assert_eq!(skill.trigger, "/tests");
    assert_eq!(skill.mode, "agent");
    assert_eq!(skill.agent, Some("code_writer".to_string()));
}

#[test]
fn test_detect_language() {
    assert_eq!(detect_language("main.rs"), "rust");
    assert_eq!(detect_language("app.tsx"), "typescript");
    assert_eq!(detect_language("script.js"), "javascript");
    assert_eq!(detect_language("main.py"), "python");
    assert_eq!(detect_language("main.go"), "go");
    assert_eq!(detect_language("App.java"), "java");
    assert_eq!(detect_language("style.css"), "css");
    assert_eq!(detect_language("config.json"), "json");
    assert_eq!(detect_language("README.md"), "markdown");
    assert_eq!(detect_language("unknown.xyz"), "text");
    assert_eq!(detect_language("noext"), "text");
}

#[test]
fn test_skill_manager_load_builtins() {
    let tmp = std::env::temp_dir().join("neecoder_skill_test_1");
    let _ = std::fs::remove_dir_all(&tmp);
    let manager = SkillManager::new(tmp.clone(), None);
    let skills = manager.list();

    // Should have 5 built-in skills (auto-review added)
    assert_eq!(skills.len(), 5);
    assert!(skills.iter().any(|s| s.trigger == "/review"));
    assert!(skills.iter().any(|s| s.trigger == "/explain"));
    assert!(skills.iter().any(|s| s.trigger == "/refactor"));
    assert!(skills.iter().any(|s| s.trigger == "/tests"));
    assert!(skills.iter().any(|s| s.trigger == "/auto-review"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_skill_manager_find() {
    let tmp = std::env::temp_dir().join("neecoder_skill_test_2");
    let _ = std::fs::remove_dir_all(&tmp);
    let manager = SkillManager::new(tmp.clone(), None);

    let skill = manager.find("/review").unwrap();
    assert_eq!(skill.name, "review");
    assert_eq!(skill.mode, "agent");

    assert!(manager.find("/nonexistent").is_none());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_skill_manager_project_override() {
    let global_tmp = std::env::temp_dir().join("neecoder_skill_global");
    let project_tmp = std::env::temp_dir().join("neecoder_skill_project");
    let _ = std::fs::remove_dir_all(&global_tmp);
    let _ = std::fs::remove_dir_all(&project_tmp);

    // Create a project-level skill that overrides /review
    std::fs::create_dir_all(&project_tmp).unwrap();
    let custom_review = r#"---
name: review
description: Custom review
trigger: /review
mode: ask
---

Custom review template."#;
    std::fs::write(project_tmp.join("review.md"), custom_review).unwrap();

    let manager = SkillManager::new(global_tmp.clone(), Some(project_tmp.clone()));
    let skill = manager.find("/review").unwrap();
    assert_eq!(skill.description, "Custom review");
    assert_eq!(skill.mode, "ask"); // Overridden from "agent" to "ask"

    let _ = std::fs::remove_dir_all(&global_tmp);
    let _ = std::fs::remove_dir_all(&project_tmp);
}

#[test]
fn test_skill_manager_reload() {
    let tmp = std::env::temp_dir().join("neecoder_skill_reload");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let manager = SkillManager::new(tmp.clone(), None);
    assert_eq!(manager.list().len(), 5); // builtins only (auto-review included)

    // Add a new skill file
    let new_skill = r#"---
name: custom
description: Custom skill
trigger: /custom
mode: ask
---

Do custom thing."#;
    std::fs::write(tmp.join("custom.md"), new_skill).unwrap();

    // Reload
    manager.reload();
    let skills = manager.list();
    assert_eq!(skills.len(), 6); // 5 builtins + 1 custom
    assert!(skills.iter().any(|s| s.trigger == "/custom"));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_ensure_default_files() {
    let tmp = std::env::temp_dir().join("neecoder_skill_ensure");
    let _ = std::fs::remove_dir_all(&tmp);

    let manager = SkillManager::new(tmp.clone(), None);
    manager.ensure_default_files();

    assert!(tmp.join("review.md").exists());
    assert!(tmp.join("explain.md").exists());
    assert!(tmp.join("refactor.md").exists());
    assert!(tmp.join("tests.md").exists());

    let _ = std::fs::remove_dir_all(&tmp);
}
