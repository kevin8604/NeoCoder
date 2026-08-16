use super::{Tool, ToolContext};
use crate::agent::utils::SKIP_DIRS;
use crate::agent::utils::resolve_path;

pub struct ListDirectory;

#[async_trait::async_trait]
impl Tool for ListDirectory {
    fn name(&self) -> &str {
        "list_directory"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or(".");
        let path = resolve_path(ctx.project_path.as_deref(), raw);
        let recursive = args["recursive"].as_bool().unwrap_or(false);
        let max_depth = args["max_depth"]
            .as_u64()
            .unwrap_or(if recursive { 3 } else { 1 }) as usize;
        let filter = args["filter"].as_str().unwrap_or("").to_lowercase();

        if let Err(e) = ctx
            .sandbox
            .check_path(&path, ctx.project_path.as_deref(), false)
        {
            return format!("Error: Sandbox blocked: {}", e);
        }

        let skip_set: std::collections::HashSet<&str> = SKIP_DIRS.iter().copied().collect();
        let mut output = String::new();
        let mut file_count = 0usize;
        let mut dir_count = 0usize;
        let mut total_size = 0u64;

        list_dir_recursive(
            &path,
            0,
            max_depth,
            &filter,
            &skip_set,
            &mut output,
            &mut file_count,
            &mut dir_count,
            &mut total_size,
        );

        if output.is_empty() {
            return format!(
                "Directory {} is empty or all entries filtered out",
                path.display()
            );
        }

        let size_str = format_size(total_size);
        let header = format!(
            "Directory listing for {} ({} files, {} dirs, {} total):\n",
            path.display(),
            file_count,
            dir_count,
            size_str
        );
        format!("{}{}", header, output)
    }
}

fn list_dir_recursive(
    dir: &std::path::Path,
    depth: usize,
    max_depth: usize,
    filter: &str,
    skip_set: &std::collections::HashSet<&str>,
    output: &mut String,
    file_count: &mut usize,
    dir_count: &mut usize,
    total_size: &mut u64,
) {
    if depth >= max_depth {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut entry_list: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entry_list.sort_by(|a, b| {
        let a_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let b_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
        b_dir.cmp(&a_dir).then(a.file_name().cmp(&b.file_name()))
    });

    let indent = "  ".repeat(depth + 1);
    for entry in entry_list {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

        if is_dir && skip_set.contains(name.as_str()) {
            continue;
        }

        if !filter.is_empty() && !name.to_lowercase().contains(filter) {
            if is_dir && depth + 1 < max_depth {
                list_dir_recursive(
                    &entry.path(),
                    depth + 1,
                    max_depth,
                    filter,
                    skip_set,
                    output,
                    file_count,
                    dir_count,
                    total_size,
                );
            }
            continue;
        }

        if is_dir {
            *dir_count += 1;
            output.push_str(&format!("{}\u{1f4c1} {}/\n", indent, name));
            list_dir_recursive(
                &entry.path(),
                depth + 1,
                max_depth,
                filter,
                skip_set,
                output,
                file_count,
                dir_count,
                total_size,
            );
        } else {
            *file_count += 1;
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            *total_size += size;
            output.push_str(&format!(
                "{}\u{1f4c4} {} ({})\n",
                indent,
                name,
                format_size(size)
            ));
        }
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
