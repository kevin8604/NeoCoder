pub mod builtin;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ── Skill Definition ───────────────────────────────────────────────────────

/// A single Skill definition parsed from a Markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub trigger: String,
    pub mode: String,
    pub agent: Option<String>,
    pub tools: Option<Vec<String>>,
    pub template: String,
}

/// YAML frontmatter metadata (deserialized via serde_yaml).
#[derive(Debug, Deserialize)]
struct SkillMeta {
    name: String,
    description: String,
    trigger: String,
    #[serde(default = "default_mode")]
    mode: String,
    agent: Option<String>,
    tools: Option<Vec<String>>,
}

fn default_mode() -> String {
    "ask".to_string()
}

// ── Template Variables ─────────────────────────────────────────────────────

/// Variables available for substitution in Skill templates.
#[derive(Debug, Clone, Default)]
pub struct SkillVars {
    pub selection: String,
    pub file_path: String,
    pub file_content: String,
    pub project_path: String,
    pub arguments: String,
    pub language: String,
}

impl SkillVars {
    /// Substitute all `$VARIABLE` placeholders in the template.
    pub fn render(&self, template: &str) -> String {
        template
            .replace("$SELECTION", &self.selection)
            .replace("$FILE_PATH", &self.file_path)
            .replace("$FILE_CONTENT", &self.file_content)
            .replace("$PROJECT_PATH", &self.project_path)
            .replace("$ARGUMENTS", &self.arguments)
            .replace("$LANGUAGE", &self.language)
    }
}

// ── Skill Manager ──────────────────────────────────────────────────────────

/// Manages loading, storing, and querying Skills.
/// Thread-safe via internal Mutex.
pub struct SkillManager {
    skills: Mutex<Vec<SkillDefinition>>,
    global_dir: PathBuf,
    project_dir: Option<PathBuf>,
}

impl SkillManager {
    /// Create a new SkillManager with the given global and optional project directories.
    pub fn new(global_dir: PathBuf, project_dir: Option<PathBuf>) -> Self {
        let manager = Self {
            skills: Mutex::new(Vec::new()),
            global_dir,
            project_dir,
        };
        manager.reload();
        manager
    }

    /// Reload all Skills from disk. Project-level skills override global ones (by trigger).
    pub fn reload(&self) {
        let mut all_skills: HashMap<String, SkillDefinition> = HashMap::new();

        // 1. Load built-in skills first (lowest priority)
        for (_filename, content) in builtin::builtin_skills() {
            if let Ok(skill) = parse_skill(content) {
                all_skills.insert(skill.trigger.clone(), skill);
            }
        }

        // 2. Load global skills (medium priority)
        load_skills_from_dir(&self.global_dir, &mut all_skills);

        // 3. Load project-level skills (highest priority)
        if let Some(ref proj_dir) = self.project_dir {
            load_skills_from_dir(proj_dir, &mut all_skills);
        }

        let mut skills: Vec<SkillDefinition> = all_skills.into_values().collect();
        skills.sort_by(|a, b| a.trigger.cmp(&b.trigger));

        if let Ok(mut guard) = self.skills.lock() {
            *guard = skills;
        }
    }

    /// List all available Skills.
    pub fn list(&self) -> Vec<SkillDefinition> {
        self.skills
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Find a Skill by its trigger command (e.g., "/review").
    pub fn find(&self, trigger: &str) -> Option<SkillDefinition> {
        self.skills
            .lock()
            .ok()
            .and_then(|guard| guard.iter().find(|s| s.trigger == trigger).cloned())
    }

    /// Ensure default skill files exist on disk in the global directory.
    /// Called once at startup.
    pub fn ensure_default_files(&self) {
        if !self.global_dir.exists() {
            let _ = std::fs::create_dir_all(&self.global_dir);
        }
        for (filename, content) in builtin::builtin_skills() {
            let path = self.global_dir.join(filename);
            if !path.exists() {
                if let Err(e) = std::fs::write(&path, content) {
                    log::warn!("Failed to write default skill {}: {}", path.display(), e);
                }
            }
        }
    }
}

// ── Parsing ────────────────────────────────────────────────────────────────

/// Parse a Skill from its Markdown + YAML frontmatter text.
pub fn parse_skill(content: &str) -> Result<SkillDefinition, String> {
    let content = content.trim();

    // Must start with ---
    if !content.starts_with("---") {
        return Err("Skill file must start with YAML frontmatter (---)".to_string());
    }

    // Find the closing ---
    let rest = &content[3..]; // skip opening ---
    let end = rest
        .find("\n---")
        .ok_or_else(|| "Missing closing --- in YAML frontmatter".to_string())?;

    let yaml_str = &rest[..end];
    let template = rest[end + 4..].trim_start_matches('\n').to_string();

    let meta: SkillMeta = serde_yaml::from_str(yaml_str)
        .map_err(|e| format!("Failed to parse YAML frontmatter: {}", e))?;

    Ok(SkillDefinition {
        name: meta.name,
        description: meta.description,
        trigger: meta.trigger,
        mode: meta.mode,
        agent: meta.agent,
        tools: meta.tools,
        template,
    })
}

/// Load all `.md` skill files from a directory into the provided map.
/// Later loads override earlier ones (by trigger).
fn load_skills_from_dir(dir: &Path, map: &mut HashMap<String, SkillDefinition>) {
    if !dir.exists() {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to read skill directory {}: {}", dir.display(), e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Failed to read skill file {}: {}", path.display(), e);
                continue;
            }
        };

        match parse_skill(&content) {
            Ok(skill) => {
                log::info!("Loaded skill '{}' from {}", skill.trigger, path.display());
                map.insert(skill.trigger.clone(), skill);
            }
            Err(e) => {
                log::warn!("Failed to parse skill file {}: {}", path.display(), e);
            }
        }
    }
}

/// Detect language from file path extension.
pub fn detect_language(file_path: &str) -> String {
    let ext = file_path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" => "scala",
        "html" | "htm" => "html",
        "css" | "scss" | "less" => "css",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "sh" | "bash" => "bash",
        "sql" => "sql",
        "vue" => "vue",
        "svelte" => "svelte",
        _ => "text",
    }
    .to_string()
}
