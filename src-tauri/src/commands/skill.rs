use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

use crate::skill::{SkillDefinition, SkillManager, SkillVars, detect_language};

/// Global SkillManager state
pub type SkillState = Arc<SkillManager>;

/// Parameters for executing a skill
#[derive(serde::Deserialize)]
pub struct ExecuteSkillParams {
    pub trigger: String,
    pub selection: Option<String>,
    pub file_path: Option<String>,
    pub file_content: Option<String>,
    pub project_path: Option<String>,
    pub arguments: Option<String>,
}

/// Result of executing a skill
#[derive(serde::Serialize)]
pub struct ExecuteSkillResult {
    pub rendered_message: String,
    pub mode: String,
    pub agent: Option<String>,
}

/// List all available skills.
#[tauri::command]
pub async fn list_skills(
    skill_manager: State<'_, SkillState>,
) -> Result<Vec<SkillDefinition>, String> {
    Ok(skill_manager.list())
}

/// Execute a skill: render its template with variables and return the
/// expanded message + mode/agent info. The frontend then sends this
/// message via the normal send_message flow.
#[tauri::command]
pub async fn execute_skill(
    skill_manager: State<'_, SkillState>,
    params: ExecuteSkillParams,
) -> Result<ExecuteSkillResult, String> {
    let skill = skill_manager
        .find(&params.trigger)
        .ok_or_else(|| format!("Skill '{}' not found", params.trigger))?;

    // Determine language from file path
    let language = params
        .file_path
        .as_deref()
        .map(detect_language)
        .unwrap_or_default();

    let vars = SkillVars {
        selection: params.selection.unwrap_or_default(),
        file_path: params.file_path.unwrap_or_default(),
        file_content: params.file_content.unwrap_or_default(),
        project_path: params.project_path.unwrap_or_default(),
        arguments: params.arguments.unwrap_or_default(),
        language,
    };

    let rendered_message = vars.render(&skill.template);

    Ok(ExecuteSkillResult {
        rendered_message,
        mode: skill.mode,
        agent: skill.agent,
    })
}

/// Reload skills from disk (hot reload).
#[tauri::command]
pub async fn reload_skills(
    skill_manager: State<'_, SkillState>,
) -> Result<usize, String> {
    skill_manager.reload();
    Ok(skill_manager.list().len())
}

/// Save a skill: write/update a .md file in the global skills directory,
/// then reload the skill manager.
#[tauri::command]
pub async fn save_skill(
    app: AppHandle,
    skill_manager: State<'_, SkillState>,
    skill: SkillDefinition,
) -> Result<String, String> {
    // Generate filename from trigger (strip leading '/', replace with '-', append .md)
    let filename = format!("{}.md", skill.trigger.trim_start_matches('/').replace('/', "-"));

    // Get the global skills directory
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config dir: {}", e))?;
    let skills_dir = config_dir.join("skills");

    // Ensure directory exists
    std::fs::create_dir_all(&skills_dir)
        .map_err(|e| format!("Failed to create skills dir: {}", e))?;

    // Serialize to YAML frontmatter + markdown
    let content = serialize_skill(&skill);

    let path = skills_dir.join(&filename);
    std::fs::write(&path, &content)
        .map_err(|e| format!("Failed to write skill file: {}", e))?;

    log::info!("[Skill] Saved skill '{}' to {}", skill.trigger, path.display());

    // Reload skills to pick up the change
    skill_manager.reload();

    Ok(skill.trigger)
}

/// Delete a skill by trigger name.
#[tauri::command]
pub async fn delete_skill(
    app: AppHandle,
    skill_manager: State<'_, SkillState>,
    trigger: String,
) -> Result<(), String> {
    let filename = format!("{}.md", trigger.trim_start_matches('/').replace('/', "-"));

    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config dir: {}", e))?;
    let path = config_dir.join("skills").join(&filename);

    if !path.exists() {
        return Err(format!("Skill '{}' not found", trigger));
    }

    std::fs::remove_file(&path)
        .map_err(|e| format!("Failed to delete skill file: {}", e))?;

    log::info!("[Skill] Deleted skill '{}' ({})", trigger, path.display());

    // Reload skills
    skill_manager.reload();

    Ok(())
}

/// Serialize a SkillDefinition back to YAML frontmatter + markdown.
fn serialize_skill(skill: &SkillDefinition) -> String {
    let mut yaml = String::from("---\n");
    yaml.push_str(&format!("name: {}\n", skill.name));
    yaml.push_str(&format!("description: {}\n", skill.description));
    yaml.push_str(&format!("trigger: {}\n", skill.trigger));
    yaml.push_str(&format!("mode: {}\n", skill.mode));
    if let Some(ref agent) = skill.agent {
        yaml.push_str(&format!("agent: {}\n", agent));
    }
    if let Some(ref tools) = skill.tools {
        if !tools.is_empty() {
            yaml.push_str("tools:\n");
            for t in tools {
                yaml.push_str(&format!("  - {}\n", t));
            }
        }
    }
    yaml.push_str("---\n\n");
    yaml.push_str(&skill.template);
    yaml
}
