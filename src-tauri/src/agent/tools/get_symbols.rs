use super::{Tool, ToolContext};
use crate::agent::utils::resolve_path;
use crate::lsp::detect_language;

pub struct GetSymbols;

#[async_trait::async_trait]
impl Tool for GetSymbols {
    fn name(&self) -> &str {
        "get_symbols"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["scope"].as_str().unwrap_or("");
        let scope = resolve_path(ctx.project_path.as_deref(), raw);
        let filter = args["filter"].as_str().unwrap_or("").to_lowercase();

        if let Err(e) = ctx
            .sandbox
            .check_path(&scope, ctx.project_path.as_deref(), false)
        {
            return format!("Error: Sandbox blocked: {}", e);
        }

        // 优先使用 LSP（若该语言客户端已启动），失败降级到正则启发式
        if let Some(lsp) = &ctx.lsp_manager {
            let language = detect_language(&scope.to_string_lossy());
            if lsp.has_client(&language).await {
                match lsp.get_symbols(&language, &scope.to_string_lossy()).await {
                    Ok(symbols) if !symbols.is_empty() => {
                        return format_lsp_symbols(&scope, symbols, &filter);
                    }
                    Ok(_) => {
                        return format!("No symbols found in {}", scope.display());
                    }
                    Err(e) => {
                        log::debug!(
                            "LSP get_symbols failed for {} ({}), falling back to regex",
                            scope.display(),
                            e
                        );
                    }
                }
            }
        }

        regex_symbols(&scope, &filter)
    }
}

/// 将 LSP 返回的符号格式化为与正则结果一致的文本
fn format_lsp_symbols(
    scope: &std::path::Path,
    symbols: Vec<crate::lsp::LSPSymbol>,
    filter: &str,
) -> String {
    let mut grouped: std::collections::HashMap<String, Vec<&crate::lsp::LSPSymbol>> =
        std::collections::HashMap::new();
    for s in &symbols {
        if !filter.is_empty() && !s.name.to_lowercase().contains(filter) {
            continue;
        }
        grouped.entry(s.kind.clone()).or_default().push(s);
    }

    let total: usize = grouped.values().map(Vec::len).sum();
    if total == 0 {
        return format!("No symbols found in {}", scope.display());
    }

    let mut output = format!("Symbols in {} ({} total):\n", scope.display(), total);
    // 稳定输出顺序：函数/方法/类优先，其余按名称排序
    let mut kinds: Vec<(&String, &Vec<&crate::lsp::LSPSymbol>)> = grouped.iter().collect();
    kinds.sort_by(|a, b| {
        let rank = |k: &str| match k {
            "Function" | "Method" | "Constructor" => 0,
            "Class" | "Struct" | "Interface" | "Enum" | "Module" => 1,
            "Constant" | "Variable" | "Property" | "Field" => 2,
            _ => 3,
        };
        rank(a.0).cmp(&rank(b.0)).then_with(|| a.0.cmp(b.0))
    });

    for (kind, syms) in kinds {
        let mut syms = syms.clone();
        syms.sort_by_key(|s| s.start_line);
        output.push_str(&format!("\n  {} ({})\n", kind, syms.len()));
        for s in syms {
            // LSP 行号是 0-based，转换为 1-based 展示
            output.push_str(&format!(
                "  Ln {}: {} [{:?}]\n",
                s.start_line + 1,
                s.name,
                kind
            ));
        }
    }
    output
}

/// 正则启发式符号提取（无 LSP 时的降级路径）
fn regex_symbols(scope: &std::path::Path, filter: &str) -> String {
    match std::fs::read_to_string(scope) {
        Ok(content) => {
            let mut types: Vec<String> = Vec::new();
            let mut functions: Vec<String> = Vec::new();
            let mut imports: Vec<String> = Vec::new();
            let mut constants: Vec<String> = Vec::new();
            let mut other: Vec<String> = Vec::new();

            for (i, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                let ln = i + 1;

                // Type definitions
                if trimmed.starts_with("pub struct ")
                    || trimmed.starts_with("pub enum ")
                    || trimmed.starts_with("pub trait ")
                    || trimmed.starts_with("pub type ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("enum ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("interface ")
                    || trimmed.starts_with("type ") && !trimmed.starts_with("type alias")
                {
                    types.push(format!("  Ln {}: {}", ln, trimmed));
                }
                // Functions and methods
                else if trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("pub async fn ")
                    || trimmed.starts_with("pub(crate) fn ")
                    || trimmed.starts_with("pub(crate) async fn ")
                    || trimmed.starts_with("pub(super) fn ")
                    || trimmed.starts_with("async fn ")
                    || trimmed.starts_with("function ")
                    || trimmed.starts_with("def ")
                    || trimmed.starts_with("async def ")
                    || trimmed.starts_with("pub const fn ")
                {
                    functions.push(format!("  Ln {}: {}", ln, trimmed));
                }
                // Impl blocks
                else if trimmed.starts_with("impl ") {
                    types.push(format!("  Ln {}: {}", ln, trimmed));
                }
                // Imports/exports
                else if trimmed.starts_with("use ")
                    || trimmed.starts_with("pub use ")
                    || trimmed.starts_with("import ")
                    || trimmed.starts_with("from ")
                    || trimmed.starts_with("export ")
                    || trimmed.starts_with("require(")
                    || trimmed.starts_with("mod ")
                    || trimmed.starts_with("pub mod ")
                {
                    imports.push(format!("  Ln {}: {}", ln, trimmed));
                }
                // Constants and statics
                else if trimmed.starts_with("const ")
                    || trimmed.starts_with("pub const ")
                    || trimmed.starts_with("static ")
                    || trimmed.starts_with("pub static ")
                    || trimmed.starts_with("let ") && trimmed.contains('=')
                    || trimmed.starts_with("var ")
                    || trimmed.starts_with("final ")
                {
                    constants.push(format!("  Ln {}: {}", ln, trimmed));
                }
            }

            // Apply filter if specified
            if !filter.is_empty() {
                types.retain(|s| s.to_lowercase().contains(filter));
                functions.retain(|s| s.to_lowercase().contains(filter));
                imports.retain(|s| s.to_lowercase().contains(filter));
                constants.retain(|s| s.to_lowercase().contains(filter));
                other.retain(|s| s.to_lowercase().contains(filter));
            }

            let total =
                types.len() + functions.len() + imports.len() + constants.len() + other.len();
            if total == 0 {
                return format!("No symbols found in {}", scope.display());
            }

            let mut output = format!("Symbols in {} ({} total):\n", scope.display(), total);

            if !types.is_empty() {
                output.push_str(&format!("\n  Types ({})\n", types.len()));
                for s in &types {
                    output.push_str(s);
                    output.push('\n');
                }
            }
            if !functions.is_empty() {
                output.push_str(&format!("\n  Functions ({})\n", functions.len()));
                for s in &functions {
                    output.push_str(s);
                    output.push('\n');
                }
            }
            if !imports.is_empty() {
                output.push_str(&format!("\n  Imports/Modules ({})\n", imports.len()));
                for s in &imports {
                    output.push_str(s);
                    output.push('\n');
                }
            }
            if !constants.is_empty() {
                output.push_str(&format!("\n  Constants/Vars ({})\n", constants.len()));
                for s in &constants {
                    output.push_str(s);
                    output.push('\n');
                }
            }
            if !other.is_empty() {
                output.push_str(&format!("\n  Other ({})\n", other.len()));
                for s in &other {
                    output.push_str(s);
                    output.push('\n');
                }
            }

            output
        }
        Err(e) => format!("Error reading {}: {}", scope.display(), e),
    }
}
