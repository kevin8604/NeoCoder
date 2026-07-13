use crate::agent::utils::resolve_path;
use super::{Tool, ToolContext};

pub struct DeleteDirectory;

#[async_trait::async_trait]
impl Tool for DeleteDirectory {
    fn name(&self) -> &str {
        "delete_directory"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or("");
        let path = resolve_path(ctx.project_path.as_deref(), raw);
        if let Err(e) = ctx.sandbox.check_path(&path, ctx.project_path.as_deref(), true) {
            return format!("Error: Sandbox blocked: {}", e);
        }

        // Gather info before deletion
        let mut file_count = 0usize;
        let mut total_size = 0u64;
        count_dir_contents(&path, &mut file_count, &mut total_size);

        let size_str = if total_size < 1024 {
            format!("{}B", total_size)
        } else if total_size < 1024 * 1024 {
            format!("{:.1}KB", total_size as f64 / 1024.0)
        } else {
            format!("{:.1}MB", total_size as f64 / (1024.0 * 1024.0))
        };

        match std::fs::remove_dir_all(&path) {
            Ok(()) => format!(
                "Deleted directory: {} ({} files, {} removed)",
                path.display(), file_count, size_str
            ),
            Err(e) => format!("Error deleting directory {}: {}", path.display(), e),
        }
    }
}

fn count_dir_contents(dir: &std::path::Path, file_count: &mut usize, total_size: &mut u64) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                count_dir_contents(&path, file_count, total_size);
            } else {
                *file_count += 1;
                *total_size += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
}
