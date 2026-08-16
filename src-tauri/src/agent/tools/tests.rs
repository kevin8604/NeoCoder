#[cfg(test)]
mod tests {
    use crate::agent::tools::{
        Tool, ToolContext,
        read_file::ReadFile, write_file::WriteFile, edit::Edit,
        git_status::GitStatus, git_diff::GitDiff, git_commit::GitCommit,
        memory_search::MemorySearch, coverage::CoverageTool,
        a2a_invoke::{A2aInvoke, resolve_agent_url},
    };
    use crate::sandbox::{SandboxChecker, SandboxConfig, SandboxMode};
    use serde_json::json;
    use std::sync::Arc;

    fn create_test_context(project_path: Option<&str>) -> ToolContext {
        ToolContext {
            project_path: project_path.map(|s| s.to_string()),
            indexer: None,
            sandbox: Arc::new(SandboxChecker::new(
                SandboxConfig {
                    mode: SandboxMode::Permissive,
                    // 测试在系统临时目录读写文件：显式放行临时目录，
                    // 否则 Permissive 模式会拒绝所有写入，工具逻辑无法被验证
                    allowed_paths: vec![std::env::temp_dir().to_string_lossy().to_string()],
                    ..Default::default()
                },
                None,
            )),
            lsp_manager: None,
            app_handle: None,
            session_id: None,
            tavily_api_key: String::new(),
            llm_provider: crate::config::LlmProvider::DeepSeek,
            llm_api_key: String::new(),
            llm_base_url: None,
            llm_model: String::new(),
        }
    }

    // ── ReadFile Tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_read_file_success() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_read");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("test.txt");
        std::fs::write(&test_file, "Hello, World!").unwrap();

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap()
        });

        let result = ReadFile.execute(args, &ctx).await;
        assert!(result.contains("Hello, World!"));
        assert!(result.contains(test_file.to_str().unwrap()));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let ctx = create_test_context(None);
        let args = json!({
            "path": "/nonexistent/file.txt"
        });

        let result = ReadFile.execute(args, &ctx).await;
        assert!(result.contains("Error"));
        assert!(result.contains("nonexistent"));
    }

    #[tokio::test]
    async fn test_read_file_relative_path() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_read_rel");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("relative.txt");
        std::fs::write(&test_file, "Relative path content").unwrap();

        let ctx = create_test_context(temp_dir.to_str());
        let args = json!({
            "path": "relative.txt"
        });

        let result = ReadFile.execute(args, &ctx).await;
        assert!(result.contains("Relative path content"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_read_file_empty_path() {
        let ctx = create_test_context(Some("/tmp"));
        let args = json!({
            "path": ""
        });

        let result = ReadFile.execute(args, &ctx).await;
        // Should try to read the directory itself or fail gracefully
        assert!(result.contains("Error") || result.contains("Is a directory"));
    }

    // ── WriteFile Tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_write_file_new_file() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_write");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("new_file.txt");

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": "New file content"
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully created"));
        assert!(result.contains("bytes")); // length of "New file content"

        // Verify file was created
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "New file content");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_overwrite() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_write_overwrite");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("overwrite.txt");
        std::fs::write(&test_file, "Old content").unwrap();

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": "New content"
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully overwrote"));

        // Verify file was overwritten
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "New content");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_create_parent_dirs() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_write_nested");
        // 清理可能的残留（Windows 上 remove_dir_all 可能失败被忽略，导致文件已存在）
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("nested").join("dirs").join("file.txt");

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": "Nested content"
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully created"), "result: {}", result);

        // Verify file was created in nested directory
        assert!(test_file.exists());
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "Nested content");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_empty_contents() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_write_empty");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("empty.txt");

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": ""
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully created"));
        assert!(result.contains("0 bytes"));

        // Verify empty file was created
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_relative_path() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_write_rel");
        // 清理可能的残留（Windows 上 remove_dir_all 可能失败被忽略，导致文件已存在）
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let ctx = create_test_context(temp_dir.to_str());
        let args = json!({
            "path": "relative_file.txt",
            "contents": "Relative write"
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully created"), "result: {}", result);

        // Verify file was created in project path
        let test_file = temp_dir.join("relative_file.txt");
        assert!(test_file.exists());
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "Relative write");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_unicode_content() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_write_unicode");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("unicode.txt");

        let unicode_content = "你好世界 🌍 Emoji test 中文测试";
        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": unicode_content
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully created"));

        // Verify unicode content was written correctly
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, unicode_content);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ── Edit Tests ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_edit_success() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_edit");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("edit.txt");
        std::fs::write(&test_file, "Hello World\nSecond line\nThird line").unwrap();

        let ctx = create_test_context(None);
        let args = json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "Hello World",
            "new_string": "Hello Rust"
        });

        let result = Edit.execute(args, &ctx).await;
        assert!(result.contains("Successfully edited"));

        // Verify edit was applied
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "Hello Rust\nSecond line\nThird line");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_edit_old_string_not_found() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_edit_notfound");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("notfound.txt");
        std::fs::write(&test_file, "Hello World").unwrap();

        let ctx = create_test_context(None);
        let args = json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "NonExistent",
            "new_string": "Replacement"
        });

        let result = Edit.execute(args, &ctx).await;
        assert!(result.contains("Error"));
        assert!(result.contains("not found"));

        // Verify file was not modified
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "Hello World");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_edit_multiple_occurrences() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_edit_multi");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("multi.txt");
        std::fs::write(&test_file, "foo\nbar\nfoo\nbaz").unwrap();

        let ctx = create_test_context(None);
        let args = json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "replaced"
        });

        let result = Edit.execute(args, &ctx).await;
        assert!(result.contains("Error"));
        assert!(result.contains("appears 2 times"));

        // Verify file was not modified (safety)
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "foo\nbar\nfoo\nbaz");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_edit_with_context() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_edit_ctx");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("context.txt");
        let original = "fn foo() {\n    let x = 1;\n}\n\nfn bar() {\n    let x = 1;\n}";
        std::fs::write(&test_file, original).unwrap();

        let ctx = create_test_context(None);
        let args = json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "fn foo() {\n    let x = 1;\n}",
            "new_string": "fn foo() {\n    let x = 42;\n}"
        });

        let result = Edit.execute(args, &ctx).await;
        assert!(result.contains("Successfully edited"));

        // Verify only first occurrence was replaced
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "fn foo() {\n    let x = 42;\n}\n\nfn bar() {\n    let x = 1;\n}");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_edit_empty_old_string() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_edit_empty");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("empty_old.txt");
        std::fs::write(&test_file, "Some content").unwrap();

        let ctx = create_test_context(None);
        let args = json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "",
            "new_string": "Replacement"
        });

        let result = Edit.execute(args, &ctx).await;
        assert!(result.contains("Error"));
        assert!(result.contains("required"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let ctx = create_test_context(None);
        let args = json!({
            "file_path": "/nonexistent/file.txt",
            "old_string": "something",
            "new_string": "replacement"
        });

        let result = Edit.execute(args, &ctx).await;
        assert!(result.contains("Error"));
    }

    #[tokio::test]
    async fn test_edit_whitespace_sensitive() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_edit_ws");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("whitespace.txt");
        // Note the indentation - two similar lines with different indentation
        std::fs::write(&test_file, "    let x = 1;\nlet x = 1;").unwrap();

        let ctx = create_test_context(None);
        
        // Try to match without proper indentation - should match the second occurrence
        let args = json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "let x = 1;",  // Matches twice
            "new_string": "let x = 42;"
        });

        let result = Edit.execute(args, &ctx).await;
        // Should fail because it matches twice (once with spaces, once without)
        assert!(result.contains("Error") && result.contains("appears 2 times"));

        // Now try with correct whitespace - unique match
        let args_correct = json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "    let x = 1;",
            "new_string": "    let x = 42;"
        });

        let result_correct = Edit.execute(args_correct, &ctx).await;
        assert!(result_correct.contains("Successfully edited"));

        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "    let x = 42;\nlet x = 1;");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ── CoverageTool Tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_coverage_uncovered_reads_cached_report() {
        let dir = std::env::temp_dir().join("neocoder_cov_uncovered");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("target")).unwrap();

        // 预置一个缓存报告（模拟 scan 后的产物）
        let cache = serde_json::json!({
            "scanned_at": "2026-08-09 00:00:00",
            "total_lines": 100,
            "covered_lines": 60,
            "files": [{
                "filename": format!("{}/src/agent/hooks.rs", dir.to_str().unwrap()),
                "total_lines": 50,
                "covered_lines": 20,
                "uncovered_ranges": [[1, 10], [20, 30]]
            }]
        });
        std::fs::write(
            dir.join("target").join("coverage_report.json"),
            serde_json::to_string(&cache).unwrap(),
        )
        .unwrap();

        let ctx = create_test_context(Some(dir.to_str().unwrap()));
        let result = CoverageTool.execute(json!({ "action": "uncovered" }), &ctx).await;
        assert!(result.contains("60.0% lines covered"), "result: {}", result);
        assert!(result.contains("agent/hooks.rs"));
        assert!(result.contains("1-10"));
        assert!(result.contains("Guidance"));

        // path 过滤
        let result = CoverageTool
            .execute(json!({ "action": "uncovered", "path": "nope" }), &ctx)
            .await;
        assert!(result.contains("No files match"), "result: {}", result);

        // status 显示缓存信息
        let result = CoverageTool.execute(json!({ "action": "status" }), &ctx).await;
        assert!(result.contains("Coverage cache"), "result: {}", result);
        assert!(result.contains("60.0%"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_coverage_scan_reuses_cache_without_force() {
        let dir = std::env::temp_dir().join("neocoder_cov_reuse");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("target")).unwrap();

        let cache = serde_json::json!({
            "scanned_at": "2026-08-09 00:00:00",
            "total_lines": 10,
            "covered_lines": 5,
            "files": [{
                "filename": format!("{}/src/a.rs", dir.to_str().unwrap()),
                "total_lines": 10,
                "covered_lines": 5,
                "uncovered_ranges": [[6, 10]]
            }]
        });
        std::fs::write(
            dir.join("target").join("coverage_report.json"),
            serde_json::to_string(&cache).unwrap(),
        )
        .unwrap();

        let ctx = create_test_context(Some(dir.to_str().unwrap()));
        // 有缓存且未传 force → 复用缓存，不触发 llvm-cov 执行
        let result = CoverageTool.execute(json!({ "action": "scan" }), &ctx).await;
        assert!(result.contains("cached"), "result: {}", result);
        assert!(result.contains("src/a.rs"));
        assert!(result.contains("force:true"), "result: {}", result);

        // 无缓存 → 提示先 scan（uncovered 报错路径）
        let empty = std::env::temp_dir().join("neocoder_cov_empty");
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        let ctx_empty = create_test_context(Some(empty.to_str().unwrap()));
        let result = CoverageTool
            .execute(json!({ "action": "uncovered" }), &ctx_empty)
            .await;
        assert!(result.contains("no cached coverage report"), "result: {}", result);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[tokio::test]
    async fn test_coverage_status_no_cache() {
        let dir = std::env::temp_dir().join("neocoder_cov_status");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let ctx = create_test_context(Some(dir.to_str().unwrap()));
        let result = CoverageTool.execute(json!({ "action": "status" }), &ctx).await;
        assert!(result.contains("no report yet"), "result: {}", result);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Edge Cases & Integration Tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_write_then_read_roundtrip() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_roundtrip");
        // 清理可能的残留（Windows 上 remove_dir_all 可能失败被忽略，导致文件已存在）
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("roundtrip.txt");
        let original_content = "Line 1\nLine 2\nLine 3\n特殊字符: 你好🌍";

        // Write
        let ctx = create_test_context(None);
        let write_args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": original_content
        });
        let write_result = WriteFile.execute(write_args, &ctx).await;
        assert!(write_result.contains("Successfully created"), "result: {}", write_result);

        // Read
        let read_args = json!({
            "path": test_file.to_str().unwrap()
        });
        let read_result = ReadFile.execute(read_args, &ctx).await;
        // read_file 输出带行号前缀（"   1\tLine 1"），逐行断言内容
        for line in original_content.lines() {
            assert!(
                read_result.contains(line),
                "missing {:?} in read result: {}",
                line,
                read_result
            );
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_then_edit_chain() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_chain");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("chain.txt");

        let ctx = create_test_context(None);

        // Step 1: Write initial content
        let write_args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": "fn main() {\n    println!(\"Hello\");\n}"
        });
        WriteFile.execute(write_args, &ctx).await;

        // Step 2: Edit the content
        let edit_args = json!({
            "file_path": test_file.to_str().unwrap(),
            "old_string": "println!(\"Hello\")",
            "new_string": "println!(\"Hello, World!\")"
        });
        let edit_result = Edit.execute(edit_args, &ctx).await;
        assert!(edit_result.contains("Successfully edited"));

        // Step 3: Read to verify
        let read_args = json!({
            "path": test_file.to_str().unwrap()
        });
        let read_result = ReadFile.execute(read_args, &ctx).await;
        assert!(read_result.contains("Hello, World!"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_large_file_write() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_large");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("large.txt");

        // Create 1MB content
        let large_content = "A".repeat(1024 * 1024);

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": &large_content
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully created"));
        assert!(result.contains("1048576 bytes"));

        // Verify
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content.len(), 1024 * 1024);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ── Task 3: Git Tools Tests ──────────────────────────────────────────

    #[tokio::test]
    async fn test_git_status_in_non_repo() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_git_norepo");
        let _ = std::fs::create_dir_all(&temp_dir);

        let ctx = create_test_context(temp_dir.to_str());
        let result = GitStatus.execute(json!({}), &ctx).await;
        assert!(result.contains("failed") || result.contains("not a git repository") || result.contains("timed out"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_git_diff_in_non_repo() {
        let temp_dir = std::env::temp_dir().join("neocoder_test_gitdiff_norepo");
        let _ = std::fs::create_dir_all(&temp_dir);

        let ctx = create_test_context(temp_dir.to_str());
        let result = GitDiff.execute(json!({}), &ctx).await;
        assert!(result.contains("failed") || result.contains("timed out"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_git_commit_no_message() {
        let ctx = create_test_context(None);
        let result = GitCommit.execute(json!({}), &ctx).await;
        assert!(result.contains("commit message is required"));
    }

    // ── Task 2: Memory Search Tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_memory_search_no_query() {
        let ctx = create_test_context(None);
        let result = MemorySearch.execute(json!({}), &ctx).await;
        assert!(result.contains("search query is required"));
    }

    #[tokio::test]
    async fn test_memory_search_no_app_handle() {
        let ctx = create_test_context(None);
        let result = MemorySearch.execute(json!({"query": "test"}), &ctx).await;
        assert!(result.contains("app handle not available") || result.contains("chat state not available"));
    }

    // ── Schema Consistency Tests ──────────────────────────────────────────

    /// 双源同步校验：每个注册工具都必须在 tools.json 中有对应 schema，
    /// 反之亦然（MCP 动态工具除外）。防止 tools.json 与 Rust 实现漂移。
    #[test]
    fn test_tool_schema_coverage() {
        let registry: Vec<crate::agent::ToolDefinition> =
            serde_json::from_str(include_str!("../../../tools.json")).unwrap_or_default();
        let schema_names: std::collections::HashSet<String> =
            registry.iter().map(|t| t.name.clone()).collect();

        let executor = crate::agent::tools::build_executor();
        let registered: std::collections::HashSet<String> =
            executor.registered_names().into_iter().collect();

        // 注册了但 schema 缺失（如曾发生 generate_diagram 不可见问题）
        let missing_schema: Vec<&String> = registered
            .difference(&schema_names)
            .filter(|n| !n.starts_with("mcp__")) // MCP 动态工具不在 tools.json
            .collect();
        assert!(
            missing_schema.is_empty(),
            "Tools registered in Rust but missing in tools.json: {:?}",
            missing_schema
        );

        // schema 存在但未注册（孤儿定义）
        let orphan_schema: Vec<&String> = schema_names
            .difference(&registered)
            .filter(|n| !n.starts_with("mcp__"))
            .collect();
        assert!(
            orphan_schema.is_empty(),
            "Tools defined in tools.json but not registered in Rust: {:?}",
            orphan_schema
        );
    }

    /// 每个 schema 必须是合法的 OpenAI function 格式
    #[test]
    fn test_tool_schema_valid_json() {
        let registry: Vec<crate::agent::ToolDefinition> =
            serde_json::from_str(include_str!("../../../tools.json"))
                .expect("tools.json must be valid JSON array of ToolDefinition");
        assert!(!registry.is_empty());
        for tool in &registry {
            assert!(!tool.name.is_empty(), "tool name must not be empty");
            assert!(!tool.description.is_empty(), "tool '{}' has empty description", tool.name);
            let schema = tool.to_openai_tool();
            assert_eq!(schema["type"], "function", "tool '{}' must be a function type", tool.name);
            assert!(
                schema["function"]["parameters"]["type"] == "object",
                "tool '{}' parameters must be an object schema",
                tool.name
            );
        }
    }

    // ── A2A Invoke 集成测试 ──────────────────────────────────────────────

    /// 本地 mock A2A server：Agent Card + message/send + tasks/get（立即 completed）
    async fn spawn_a2a_mock() -> String {
        use axum::{
            Json, Router,
            http::HeaderMap,
            response::IntoResponse,
            routing::{get, post},
        };
        use crate::a2a::{JsonRpcResponse, RpcError, Task, TaskState, TaskStatus};

        fn task(id: &str, state: TaskState) -> serde_json::Value {
            serde_json::to_value(Task::new(id.to_string(), TaskStatus::new(state))).unwrap()
        }

        let app = Router::new()
            .route(
                "/.well-known/agent.json",
                get(|headers: HeaderMap| async move {
                    let host = headers
                        .get("host")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("127.0.0.1:0")
                        .to_string();
                    Json(json!({
                        "name": "MockRemote",
                        "description": "mock remote agent",
                        "url": format!("http://{}/a2a", host),
                        "version": "1.0.0",
                        "capabilities": { "streaming": true, "pushNotifications": false, "stateTransitionHistory": false },
                        "skills": [{ "id": "m1", "name": "M1", "description": "d" }]
                    }))
                }),
            )
            .route(
                "/a2a",
                post(|body: String| async move {
                    let req: serde_json::Value = serde_json::from_str(&body).unwrap();
                    match req["method"].as_str().unwrap() {
                        "message/send" | "tasks/get" => {
                            let mut t = Task::new("t-a2a", TaskStatus::new(TaskState::Completed));
                            t.artifacts = vec![crate::a2a::Artifact {
                                name: "result.txt".into(),
                                parts: vec![crate::a2a::Part::Text {
                                    text: "mock result text".into(),
                                }],
                                metadata: None,
                            }];
                            Json(JsonRpcResponse::ok(
                                json!(1),
                                serde_json::to_value(&t).unwrap(),
                            ))
                            .into_response()
                        }
                        _ => Json(JsonRpcResponse::err(
                            json!(1),
                            RpcError {
                                code: -32601,
                                message: "method not found".into(),
                                data: None,
                            },
                        ))
                        .into_response(),
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{}", addr)
    }

    /// mock SSE A2A server：resubscribe 返回 working → completed 事件流
    async fn spawn_a2a_stream_mock() -> String {
        use axum::{
            Json, Router,
            http::HeaderMap,
            response::{
                IntoResponse,
                sse::{Event, KeepAlive, Sse},
            },
            routing::{get, post},
        };
        use crate::a2a::{JsonRpcResponse, Task, TaskState, TaskStatus};
        use std::convert::Infallible;
        use axum::http::StatusCode;

        fn task(id: &str, state: TaskState) -> serde_json::Value {
            serde_json::to_value(Task::new(id.to_string(), TaskStatus::new(state))).unwrap()
        }

        let app = Router::new()
            .route(
                "/.well-known/agent.json",
                get(|headers: HeaderMap| async move {
                    let host = headers
                        .get("host")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("127.0.0.1:0")
                        .to_string();
                    Json(json!({
                        "name": "StreamAgent",
                        "description": "mock",
                        "url": format!("http://{}/a2a", host),
                        "version": "1.0.0",
                        "capabilities": { "streaming": true, "pushNotifications": false, "stateTransitionHistory": false },
                        "skills": []
                    }))
                }),
            )
            .route(
                "/a2a",
                post(|body: String| async move {
                    let req: serde_json::Value = serde_json::from_str(&body).unwrap();
                    match req["method"].as_str().unwrap() {
                        "message/send" => Json(JsonRpcResponse::ok(
                            json!(1),
                            task("s1", TaskState::Working),
                        ))
                        .into_response(),
                        "tasks/resubscribe" => {
                            let stream = tokio_stream::iter(vec![
                                Ok::<Event, Infallible>(
                                    Event::default()
                                        .event("task_update")
                                        .data(task("s1", TaskState::Working).to_string()),
                                ),
                                Ok::<Event, Infallible>(
                                    Event::default()
                                        .event("task_update")
                                        .data(task("s1", TaskState::Completed).to_string()),
                                ),
                            ]);
                            Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
                        }
                        _ => StatusCode::NOT_FOUND.into_response(),
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn test_a2a_invoke_success() {
        let base = spawn_a2a_mock().await;
        let ctx = create_test_context(None);
        let result = A2aInvoke
            .execute(json!({ "url": base, "task": "do the thing" }), &ctx)
            .await;
        assert!(result.contains("MockRemote"), "{}", result);
        assert!(result.contains("mock result text"), "{}", result);
        assert!(result.contains("completed"), "{}", result);
    }

    #[tokio::test]
    async fn test_a2a_invoke_stream_mode() {
        let base = spawn_a2a_stream_mock().await;
        let ctx = create_test_context(None);
        let result = A2aInvoke
            .execute(json!({ "url": base, "task": "stream it", "mode": "stream" }), &ctx)
            .await;
        assert!(result.contains("StreamAgent"), "{}", result);
        assert!(result.contains("Completed"), "{}", result);
    }

    #[tokio::test]
    async fn test_a2a_invoke_missing_url_and_agent() {
        let ctx = create_test_context(None);
        // url 和 agent 都缺 → 报错
        let result = A2aInvoke
            .execute(json!({ "task": "x" }), &ctx)
            .await;
        assert!(result.contains("url or agent parameter is required"), "{}", result);
        // 只给 agent（未配置）→ 报错并提示配置
        let result = A2aInvoke
            .execute(json!({ "agent": "ghost", "task": "x" }), &ctx)
            .await;
        assert!(result.contains("unknown remote agent 'ghost'"), "{}", result);
        assert!(result.contains("Remote Agents"), "{}", result);
    }

    #[test]
    fn test_a2a_invoke_resolve_agent_url() {
        use crate::a2a::A2aAgentConfig;
        let agents = vec![
            A2aAgentConfig {
                name: "local-orchestrator".into(),
                url: "http://127.0.0.1:41234".into(),
                description: "d".into(),
            },
            A2aAgentConfig {
                name: "peer-1".into(),
                url: "http://127.0.0.1:51234".into(),
                description: "".into(),
            },
        ];
        assert_eq!(
            resolve_agent_url("local-orchestrator", &agents).unwrap(),
            "http://127.0.0.1:41234"
        );
        // 未命中 → 错误含可用列表
        let err = resolve_agent_url("ghost", &agents).unwrap_err();
        assert!(err.contains("unknown remote agent 'ghost'"), "{}", err);
        assert!(err.contains("local-orchestrator"), "{}", err);
        // 空列表
        let err = resolve_agent_url("x", &[]).unwrap_err();
        assert!(err.contains("configured: none"), "{}", err);
    }

    #[tokio::test]
    async fn test_a2a_invoke_with_skill() {
        let base = spawn_a2a_mock().await;
        let ctx = create_test_context(None);
        // skill 参数可选项透传，不影响正常执行
        let result = A2aInvoke
            .execute(json!({ "url": base, "task": "x", "skill": "code_writer" }), &ctx)
            .await;
        assert!(result.contains("MockRemote"), "{}", result);
        assert!(!result.contains("Error:"), "{}", result);
    }

    #[tokio::test]
    async fn test_a2a_invoke_missing_task() {
        let ctx = create_test_context(None);
        let result = A2aInvoke
            .execute(json!({ "url": "http://127.0.0.1:1" }), &ctx)
            .await;
        assert!(result.contains("task parameter is required"), "{}", result);
    }

    #[tokio::test]
    async fn test_a2a_invoke_unreachable_url() {
        let ctx = create_test_context(None);
        let result = A2aInvoke
            .execute(json!({ "url": "http://127.0.0.1:1", "task": "x", "timeout_secs": 2 }), &ctx)
            .await;
        assert!(result.starts_with("Error: A2A invocation failed"), "{}", result);
    }

    #[tokio::test]
    async fn test_a2a_invoke_invalid_mode_falls_back_to_sync() {
        let base = spawn_a2a_mock().await;
        let ctx = create_test_context(None);
        // 非法 mode 回退 sync 并成功
        let result = A2aInvoke
            .execute(json!({ "url": base, "task": "x", "mode": "bogus" }), &ctx)
            .await;
        assert!(result.contains("MockRemote"), "{}", result);
        assert!(!result.contains("Error:"), "{}", result);
    }

    #[test]
    fn test_a2a_invoke_registration_consistency() {
        // 1) executor 注册
        let executor = crate::agent::tools::build_executor();
        assert!(executor.registered_names().contains(&"a2a_invoke".to_string()));
        // 2) tools.json 定义
        let registry: Vec<crate::agent::ToolDefinition> =
            serde_json::from_str(include_str!("../../../tools.json")).unwrap();
        assert!(registry.iter().any(|t| t.name == "a2a_invoke"));
        // 3) orchestrator agent 可用
        let agents = crate::agent::definition::default_agents();
        let orchestrator = agents
            .iter()
            .find(|a| a.id == "orchestrator")
            .expect("orchestrator agent exists");
        assert!(orchestrator.tool_names.contains(&"a2a_invoke".to_string()));
    }
}
