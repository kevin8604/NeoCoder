use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Shared checkpoint store, keyed by session_id.
/// Managed as Tauri state for cross-command access.
pub type CheckpointStore =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<Checkpoint>>>>;

/// Create a new empty CheckpointStore.
pub fn new_store() -> CheckpointStore {
    std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// A checkpoint representing the state of the project at a specific iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub iteration: u32,
    pub timestamp: i64,
    pub commit_hash: Option<String>,
    pub files: Vec<String>,
    pub description: String,
}

/// Manages checkpoints for a single Agent session.
/// Thread-safe via interior mutability (Mutex).
pub struct CheckpointManager {
    checkpoints: Mutex<Vec<Checkpoint>>,
    project_path: Option<String>,
}

impl CheckpointManager {
    pub fn new(project_path: Option<String>) -> Self {
        Self {
            checkpoints: Mutex::new(Vec::new()),
            project_path,
        }
    }

    /// Create a checkpoint by staging modified files and creating a git commit.
    pub async fn create(
        &self,
        iteration: u32,
        files: Vec<String>,
        description: String,
    ) -> Result<Checkpoint, String> {
        let timestamp = chrono::Utc::now().timestamp();
        let mut commit_hash: Option<String> = None;

        if let Some(ref work_dir) = self.project_path
            && !files.is_empty()
            && Self::is_git_repo(work_dir).await
        {
            // Stage modified files
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                tokio::process::Command::new("git")
                    .arg("add")
                    .args(&files)
                    .current_dir(work_dir)
                    .output(),
            )
            .await;

            // Create commit
            let commit_msg = format!("checkpoint: iteration {} - {}", iteration, description);
            let commit_output = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                tokio::process::Command::new("git")
                    .arg("commit")
                    .arg("-m")
                    .arg(&commit_msg)
                    .arg("--allow-empty")
                    .current_dir(work_dir)
                    .output(),
            )
            .await;

            if let Ok(Ok(out)) = commit_output
                && out.status.success()
            {
                let stdout = String::from_utf8_lossy(&out.stdout);
                commit_hash = extract_commit_hash(&stdout);
            }

            // Get full hash if we got a short one
            if let Some(ref short) = commit_hash {
                let rev_output = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    tokio::process::Command::new("git")
                        .arg("rev-parse")
                        .arg(short)
                        .current_dir(work_dir)
                        .output(),
                )
                .await;
                if let Ok(Ok(out)) = rev_output
                    && out.status.success()
                {
                    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !hash.is_empty() {
                        commit_hash = Some(hash);
                    }
                }
            }
        }

        let checkpoint = Checkpoint {
            iteration,
            timestamp,
            commit_hash,
            files: files.clone(),
            description,
        };

        let mut cps = self.checkpoints.lock().unwrap_or_else(|e| e.into_inner());
        cps.push(checkpoint.clone());
        Ok(checkpoint)
    }

    /// Restore files to a previous checkpoint state.
    pub async fn restore(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        let work_dir = self.project_path.as_ref().ok_or("No project path set")?;

        if let Some(ref hash) = checkpoint.commit_hash {
            // Use git to restore files from the checkpoint commit
            let files_args: Vec<&str> = checkpoint.files.iter().map(|s| s.as_str()).collect();

            let output = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                tokio::process::Command::new("git")
                    .arg("checkout")
                    .arg(hash)
                    .arg("--")
                    .args(&files_args)
                    .current_dir(work_dir)
                    .output(),
            )
            .await;

            match output {
                Ok(Ok(out)) => {
                    if !out.status.success() {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        return Err(format!("git checkout failed: {}", stderr.trim()));
                    }
                }
                Ok(Err(e)) => return Err(format!("Failed to execute git checkout: {}", e)),
                Err(_) => return Err("git checkout timed out".to_string()),
            }
        } else {
            // No commit hash — can't restore
            return Err("Checkpoint has no commit hash, cannot restore".to_string());
        }

        Ok(())
    }

    /// List all checkpoints.
    pub fn list(&self) -> Vec<Checkpoint> {
        let cps = self.checkpoints.lock().unwrap_or_else(|e| e.into_inner());
        cps.clone()
    }

    /// Get the latest checkpoint.
    pub fn latest(&self) -> Option<Checkpoint> {
        let cps = self.checkpoints.lock().unwrap_or_else(|e| e.into_inner());
        cps.last().cloned()
    }

    /// Check if a directory is a git repository.
    async fn is_git_repo(work_dir: &str) -> bool {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::process::Command::new("git")
                .arg("rev-parse")
                .arg("--is-inside-work-tree")
                .current_dir(work_dir)
                .output(),
        )
        .await;

        matches!(output, Ok(Ok(out)) if out.status.success())
    }
}

/// Extract the short commit hash from git commit output.
fn extract_commit_hash(output: &str) -> Option<String> {
    for line in output.lines() {
        if line.starts_with('[')
            && let Some(close) = line.find(']')
        {
            let inner = &line[1..close];
            let parts: Vec<&str> = inner.split_whitespace().collect();
            if parts.len() >= 2 {
                return Some(parts[1].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_store_create() {
        let store = new_store();
        let cp = Checkpoint {
            iteration: 0,
            timestamp: 12345,
            commit_hash: Some("abc123".to_string()),
            files: vec!["src/main.rs".to_string()],
            description: "Test checkpoint".to_string(),
        };

        {
            let mut s = store.lock().unwrap();
            s.entry("session1".to_string()).or_default().push(cp);
        }

        let s = store.lock().unwrap();
        let cps = s.get("session1").unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].iteration, 0);
        assert_eq!(cps[0].commit_hash.as_ref().unwrap(), "abc123");
    }

    #[test]
    fn test_checkpoint_store_empty_session() {
        let store = new_store();
        let s = store.lock().unwrap();
        assert!(s.get("nonexistent").is_none());
    }

    #[test]
    fn test_checkpoint_manager_list_empty() {
        let manager = CheckpointManager::new(None);
        assert!(manager.list().is_empty());
        assert!(manager.latest().is_none());
    }

    #[test]
    fn test_extract_commit_hash() {
        // Standard git commit output: "[main abc1234] message"
        let result = extract_commit_hash("[main abc1234] Test commit message");
        assert_eq!(result, Some("abc1234".to_string()));

        // No bracket line
        let result = extract_commit_hash("Some output without brackets");
        assert_eq!(result, None);
    }
}
