use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A node in the dependency graph
#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub file_type: String,
}

/// An edge in the dependency graph
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

/// The full dependency graph result
#[derive(Debug, Clone, Serialize)]
pub struct DependencyGraph {
    pub mermaid: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub node_count: usize,
    pub edge_count: usize,
}

/// Supported source file extensions for dependency analysis
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "c", "cpp", "h", "hpp",
];

/// Directories to skip during scanning
const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", ".git", "dist", "build", "__pycache__", ".venv", "vendor",
];

/// Analyze project dependencies and generate a Mermaid graph.
#[tauri::command]
pub async fn get_dependency_graph(
    project_path: String,
    depth: Option<usize>,
) -> Result<DependencyGraph, String> {
    let root = Path::new(&project_path);
    if !root.exists() || !root.is_dir() {
        return Err(format!("Invalid project path: {}", project_path));
    }

    let max_depth = depth.unwrap_or(5);

    // Collect all source files
    let mut source_files: Vec<PathBuf> = Vec::new();
    collect_source_files(root, &mut source_files, 0, max_depth);

    if source_files.is_empty() {
        return Err("No source files found in project".to_string());
    }

    // Build a map of relative path -> file info
    let mut file_map: HashMap<String, PathBuf> = HashMap::new();
    for file in &source_files {
        if let Ok(rel) = file.strip_prefix(root) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            file_map.insert(rel_str, file.clone());
        }
    }

    // Extract dependencies for each file
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut edge_set: HashSet<(String, String)> = HashSet::new();
    let mut node_ids: HashSet<String> = HashSet::new();

    for file in &source_files {
        let rel_path = match file.strip_prefix(root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        let content = match tokio::fs::read_to_string(file).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let deps = extract_imports(&content, &ext);

        node_ids.insert(rel_path.clone());

        for dep in deps {
            // Try to resolve the dependency to a project file
            if let Some(resolved) = resolve_dependency(&dep, &rel_path, &file_map, &ext) {
                if resolved != rel_path {
                    let key = (rel_path.clone(), resolved.clone());
                    if edge_set.insert(key) {
                        node_ids.insert(resolved.clone());
                        edges.push(GraphEdge {
                            from: rel_path.clone(),
                            to: resolved,
                        });
                    }
                }
            }
        }
    }

    // Build nodes list
    let nodes: Vec<GraphNode> = node_ids
        .iter()
        .map(|id| {
            let label = id.rsplit('/').next().unwrap_or(id).to_string();
            let file_type = id
                .rsplit('.')
                .next()
                .unwrap_or("unknown")
                .to_string();
            GraphNode {
                id: id.clone(),
                label,
                file_type,
            }
        })
        .collect();

    // Generate Mermaid code
    let mermaid = generate_mermaid(&nodes, &edges);

    let node_count = nodes.len();
    let edge_count = edges.len();

    Ok(DependencyGraph {
        mermaid,
        nodes,
        edges,
        node_count,
        edge_count,
    })
}

/// Recursively collect source files up to max_depth
fn collect_source_files(dir: &Path, files: &mut Vec<PathBuf>, current_depth: usize, max_depth: usize) {
    if current_depth > max_depth {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if SKIP_DIRS.contains(&dir_name) {
                continue;
            }
            collect_source_files(&path, files, current_depth + 1, max_depth);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if SOURCE_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                    files.push(path);
                }
            }
        }
    }
}

/// Extract import/use/require statements from file content based on language
fn extract_imports(content: &str, ext: &str) -> Vec<String> {
    let mut imports = Vec::new();

    match ext {
        "rs" => {
            // Rust: use crate::xxx / mod xxx
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("use crate::") {
                    // use crate::module::submodule
                    if let Some(path) = trimmed.strip_prefix("use crate::") {
                        let path = path.trim_end_matches(';').trim();
                        // Take the first segment as module
                        let module = path.split("::").next().unwrap_or(path);
                        imports.push(module.to_string());
                    }
                } else if trimmed.starts_with("mod ") {
                    // mod xxx;
                    if let Some(module) = trimmed.strip_prefix("mod ") {
                        let module = module.trim_end_matches(';').trim();
                        imports.push(module.to_string());
                    }
                }
            }
        }
        "ts" | "tsx" | "js" | "jsx" => {
            // TypeScript/JavaScript: import ... from './xxx' / require('./xxx')
            for line in content.lines() {
                let trimmed = line.trim();
                // import ... from '...' or "..."
                if trimmed.starts_with("import ") {
                    if let Some(path) = extract_quoted_path(trimmed) {
                        if path.starts_with('.') {
                            imports.push(path);
                        }
                    }
                }
                // require('...')
                if trimmed.contains("require(") {
                    if let Some(start) = trimmed.find("require(") {
                        let rest = &trimmed[start + 8..];
                        if let Some(path) = extract_first_quoted(rest) {
                            if path.starts_with('.') {
                                imports.push(path);
                            }
                        }
                    }
                }
            }
        }
        "py" => {
            // Python: from xxx import / import xxx
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("from ") {
                    if let Some(module) = trimmed.strip_prefix("from ") {
                        let module = module.split(" import").next().unwrap_or("").trim();
                        if !module.is_empty() && !module.starts_with('.') {
                            imports.push(module.replace('.', "/"));
                        } else if module.starts_with('.') {
                            imports.push(module.trim_start_matches('.').replace('.', "/"));
                        }
                    }
                } else if trimmed.starts_with("import ") {
                    if let Some(module) = trimmed.strip_prefix("import ") {
                        let module = module.split(',').next().unwrap_or("").trim();
                        let module = module.split(" as ").next().unwrap_or(module).trim();
                        if !module.is_empty() {
                            imports.push(module.replace('.', "/"));
                        }
                    }
                }
            }
        }
        "go" => {
            // Go: import "xxx" or import ( "xxx" )
            let mut in_import_block = false;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("import (") {
                    in_import_block = true;
                    continue;
                }
                if in_import_block {
                    if trimmed == ")" {
                        in_import_block = false;
                        continue;
                    }
                    if let Some(path) = extract_first_quoted(trimmed) {
                        imports.push(path);
                    }
                } else if trimmed.starts_with("import ") {
                    if let Some(path) = extract_first_quoted(trimmed) {
                        imports.push(path);
                    }
                }
            }
        }
        "java" => {
            // Java: import xxx.yyy.zzz;
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    if let Some(path) = trimmed.strip_prefix("import ") {
                        let path = path.trim_end_matches(';').trim();
                        // Convert package path to file path
                        imports.push(path.replace('.', "/"));
                    }
                }
            }
        }
        _ => {}
    }

    imports
}

/// Extract a quoted path from an import statement
fn extract_quoted_path(line: &str) -> Option<String> {
    // Find 'from' keyword then extract the quoted string after it
    if let Some(from_idx) = line.find("from") {
        let rest = &line[from_idx + 4..];
        extract_first_quoted(rest)
    } else {
        extract_first_quoted(line)
    }
}

/// Extract the first quoted string (single or double quotes)
fn extract_first_quoted(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\'' || chars[i] == '"' {
            let quote = chars[i];
            let start = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }
            if i < chars.len() {
                return Some(chars[start..i].iter().collect());
            }
        }
        i += 1;
    }
    None
}

/// Try to resolve a dependency string to a project file path
fn resolve_dependency(
    dep: &str,
    from_file: &str,
    file_map: &HashMap<String, PathBuf>,
    ext: &str,
) -> Option<String> {
    // Normalize the dependency path
    let dep_normalized = dep.replace('\\', "/");

    // For relative imports (./xxx or ../xxx)
    if dep_normalized.starts_with('.') {
        let from_dir = from_file.rsplit('/').next().map(|_| {
            from_file.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
        }).unwrap_or("");

        let resolved = normalize_relative_path(from_dir, &dep_normalized);

        // Try with various extensions
        return try_resolve_with_extensions(&resolved, ext, file_map);
    }

    // For Rust module names (crate::xxx -> xxx.rs or xxx/mod.rs)
    if ext == "rs" {
        let candidates = vec![
            format!("src/{}.rs", dep_normalized),
            format!("src/{}/mod.rs", dep_normalized),
            format!("{}.rs", dep_normalized),
            format!("{}/mod.rs", dep_normalized),
        ];
        for candidate in candidates {
            if file_map.contains_key(&candidate) {
                return Some(candidate);
            }
        }
    }

    // For Python module paths
    if ext == "py" {
        let candidates = vec![
            format!("{}.py", dep_normalized),
            format!("{}/__init__.py", dep_normalized),
        ];
        for candidate in candidates {
            if file_map.contains_key(&candidate) {
                return Some(candidate);
            }
        }
    }

    // Direct match
    if file_map.contains_key(&dep_normalized) {
        return Some(dep_normalized);
    }

    None
}

/// Try to resolve a path with various file extensions
fn try_resolve_with_extensions(
    base_path: &str,
    source_ext: &str,
    file_map: &HashMap<String, PathBuf>,
) -> Option<String> {
    // Direct match (already has extension)
    if file_map.contains_key(base_path) {
        return Some(base_path.to_string());
    }

    // Try common extensions based on source file type
    let extensions: Vec<&str> = match source_ext {
        "ts" | "tsx" => vec!["ts", "tsx", "js", "jsx", "d.ts"],
        "js" | "jsx" => vec!["js", "jsx", "ts", "tsx"],
        "py" => vec!["py"],
        "go" => vec!["go"],
        "rs" => vec!["rs"],
        _ => vec!["ts", "tsx", "js", "jsx", "py", "go", "rs"],
    };

    for ext in &extensions {
        let candidate = format!("{}.{}", base_path, ext);
        if file_map.contains_key(&candidate) {
            return Some(candidate);
        }
    }

    // Try index files (for directory imports)
    for ext in &extensions {
        let candidate = format!("{}/index.{}", base_path, ext);
        if file_map.contains_key(&candidate) {
            return Some(candidate);
        }
    }

    None
}

/// Normalize a relative path (handle ./ and ../)
fn normalize_relative_path(from_dir: &str, relative: &str) -> String {
    let mut parts: Vec<&str> = if from_dir.is_empty() {
        Vec::new()
    } else {
        from_dir.split('/').collect()
    };

    for segment in relative.split('/') {
        match segment {
            "." | "" => {}
            ".." => { parts.pop(); }
            _ => parts.push(segment),
        }
    }

    parts.join("/")
}

/// Generate Mermaid graph code from nodes and edges
fn generate_mermaid(nodes: &[GraphNode], edges: &[GraphEdge]) -> String {
    let mut lines = vec!["graph LR".to_string()];

    // Add node definitions with sanitized IDs
    for node in nodes {
        let id = sanitize_mermaid_id(&node.id);
        lines.push(format!("    {}[\"{}\"]", id, node.label));
    }

    // Add edges
    for edge in edges {
        let from = sanitize_mermaid_id(&edge.from);
        let to = sanitize_mermaid_id(&edge.to);
        lines.push(format!("    {} --> {}", from, to));
    }

    lines.join("\n")
}

/// Sanitize a string for use as a Mermaid node ID
fn sanitize_mermaid_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}
