#[cfg(test)]
mod tests {
    use crate::agent::tools::{
        Tool, ToolContext,
        read_file::ReadFile, write_file::WriteFile, edit::Edit,
        git_status::GitStatus, git_diff::GitDiff, git_commit::GitCommit,
        memory_search::MemorySearch,
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
                    ..Default::default()
                },
                None,
            )),
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_read");
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_read_rel");
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_write");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("new_file.txt");

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": "New file content"
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully wrote"));
        assert!(result.contains("bytes")); // length of "New file content"

        // Verify file was created
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "New file content");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_overwrite() {
        let temp_dir = std::env::temp_dir().join("neecoder_test_write_overwrite");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("overwrite.txt");
        std::fs::write(&test_file, "Old content").unwrap();

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": "New content"
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully wrote"));

        // Verify file was overwritten
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "New content");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_create_parent_dirs() {
        let temp_dir = std::env::temp_dir().join("neecoder_test_write_nested");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("nested").join("dirs").join("file.txt");

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": "Nested content"
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully wrote"));

        // Verify file was created in nested directory
        assert!(test_file.exists());
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "Nested content");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_empty_contents() {
        let temp_dir = std::env::temp_dir().join("neecoder_test_write_empty");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("empty.txt");

        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": ""
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully wrote"));
        assert!(result.contains("0 bytes"));

        // Verify empty file was created
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_file_relative_path() {
        let temp_dir = std::env::temp_dir().join("neecoder_test_write_rel");
        let _ = std::fs::create_dir_all(&temp_dir);

        let ctx = create_test_context(temp_dir.to_str());
        let args = json!({
            "path": "relative_file.txt",
            "contents": "Relative write"
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully wrote"));

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
        let temp_dir = std::env::temp_dir().join("neecoder_test_write_unicode");
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("unicode.txt");

        let unicode_content = "你好世界 🌍 Emoji test 中文测试";
        let ctx = create_test_context(None);
        let args = json!({
            "path": test_file.to_str().unwrap(),
            "contents": unicode_content
        });

        let result = WriteFile.execute(args, &ctx).await;
        assert!(result.contains("Successfully wrote"));

        // Verify unicode content was written correctly
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, unicode_content);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ── Edit Tests ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_edit_success() {
        let temp_dir = std::env::temp_dir().join("neecoder_test_edit");
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_edit_notfound");
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_edit_multi");
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_edit_ctx");
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_edit_empty");
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_edit_ws");
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

    // ── Edge Cases & Integration Tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_write_then_read_roundtrip() {
        let temp_dir = std::env::temp_dir().join("neecoder_test_roundtrip");
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
        assert!(write_result.contains("Successfully wrote"));

        // Read
        let read_args = json!({
            "path": test_file.to_str().unwrap()
        });
        let read_result = ReadFile.execute(read_args, &ctx).await;
        assert!(read_result.contains(original_content));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_write_then_edit_chain() {
        let temp_dir = std::env::temp_dir().join("neecoder_test_chain");
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_large");
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
        assert!(result.contains("Successfully wrote"));
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
        let temp_dir = std::env::temp_dir().join("neecoder_test_git_norepo");
        let _ = std::fs::create_dir_all(&temp_dir);

        let ctx = create_test_context(temp_dir.to_str());
        let result = GitStatus.execute(json!({}), &ctx).await;
        assert!(result.contains("failed") || result.contains("not a git repository") || result.contains("timed out"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_git_diff_in_non_repo() {
        let temp_dir = std::env::temp_dir().join("neecoder_test_gitdiff_norepo");
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
}
