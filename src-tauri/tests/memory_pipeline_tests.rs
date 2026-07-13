//! Memory pipeline integration tests — verify the end-to-end memory lifecycle:
//!   Session → Messages → Dreaming → Long-Term → Ebbinghaus → Context Injection
//!
//! These tests exercise the full memory pipeline without requiring a Tauri runtime.

use std::path::PathBuf;

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neecoder_memory_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ── Session lifecycle (CRUD) ────────────────────────────────────────────────

#[test]
fn test_session_full_lifecycle() {
    let dir = temp_dir();
    let manager = neecoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));

    // Create multiple sessions
    let s1 = manager.create_session().expect("create s1");
    let s2 = manager.create_session().expect("create s2");
    assert_ne!(s1, s2, "session IDs should be unique");

    // Add messages to both
    let user_msg = neecoder_tauri_lib::chat::ChatMessage {
        role: neecoder_tauri_lib::chat::Role::User,
        content: "msg1".into(),
        tool_calls: None,
        images: None,
    };
    let assistant_msg = neecoder_tauri_lib::chat::ChatMessage {
        role: neecoder_tauri_lib::chat::Role::Assistant,
        content: "reply1".into(),
        tool_calls: None,
        images: None,
    };
    manager.add_message(&s1, user_msg).expect("add to s1");
    manager.add_message(&s1, assistant_msg).expect("add to s1");

    // Verify s1 has 2 messages
    let ctx1 = manager.get_context_window(&s1, 100_000).expect("read s1");
    assert_eq!(ctx1.len(), 2, "s1 should have 2 messages");

    // s2 should be empty
    let ctx2 = manager.get_context_window(&s2, 100_000).expect("read s2");
    assert!(ctx2.is_empty(), "s2 should be empty");

    // Clear s1
    manager.clear_session(&s1).expect("clear s1");
    let ctx1_after = manager.get_context_window(&s1, 100_000).expect("read s1 after");
    assert!(ctx1_after.is_empty(), "s1 should be empty after clear");

    // Delete s1, verify gone from list
    manager.delete_session(&s1).expect("delete s1");
    let sessions = manager.get_all_sessions().expect("list");
    assert!(!sessions.iter().any(|s| s.id == s1), "s1 should be deleted");

    cleanup(&dir);
}

// ── Context window trimming ─────────────────────────────────────────────────

#[test]
fn test_context_window_respects_token_budget() {
    let dir = temp_dir();
    let manager = neecoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));
    let sid = manager.create_session().expect("create");

    // Add 50 messages (each ~20 chars ≈ 5 tokens)
    for i in 0..50 {
        let msg = neecoder_tauri_lib::chat::ChatMessage {
            role: neecoder_tauri_lib::chat::Role::Assistant,
            content: format!("This is message number {:04} with some extra text.", i),
            tool_calls: None,
            images: None,
        };
        manager.add_message(&sid, msg).expect("add");
    }

    // Request a small window (should trim older messages)
    let small = manager.get_context_window(&sid, 50).expect("small window");
    assert!(small.len() < 50, "small token budget should trim messages: got {}", small.len());

    // Request a large window (should return all)
    let large = manager.get_context_window(&sid, 100_000).expect("large window");
    assert_eq!(large.len(), 50, "large budget should return all messages");

    cleanup(&dir);
}

// ── Long-term memory with Ebbinghaus retention ──────────────────────────────

#[test]
fn test_long_term_append_and_recall() {
    let dir = temp_dir();
    let manager = neecoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));

    // Start with clean long_term
    manager.write_long_term("# Memory\n\n").expect("write empty");

    // Append entries with different categories
    manager.append_long_term("Lessons", "- [Lesson] Use `Result<T, E>` for error handling").expect("append");
    manager.append_long_term("Decisions", "- [Decision] Use PostgreSQL for storage").expect("append");
    manager.append_long_term("Patterns", "- [Pattern] RAII ensures resource cleanup").expect("append");

    // Read back
    let content = manager.read_long_term().expect("read");
    assert!(content.contains("[Lesson]"), "should have lesson");
    assert!(content.contains("[Decision]"), "should have decision");
    assert!(content.contains("[Pattern]"), "should have pattern");

    cleanup(&dir);
}

#[test]
fn test_ebbinghaus_retention_decay_over_time() {
    use neecoder_tauri_lib::memory::ebbinghaus::{MemoryEntry, MemoryCategory, compute_retention};

    let today = chrono::Utc::now().date_naive();

    let entry = MemoryEntry {
        id: "test-1".into(),
        key: None,
        text: "- [Lesson] Test".into(),
        created: today - chrono::Duration::days(5),
        last_recalled: today - chrono::Duration::days(5),
        recall_count: 0,
        stability: 1.0,
        section: "Test".into(),
        category: MemoryCategory::Lesson,
        session_id: None,
    };

    // After 5 days with S=1.0: R = e^(-5) ≈ 0.0067
    let r5 = compute_retention(&entry, today);
    assert!(r5 < 0.05, "retention after 5 days at S=1.0 should be very low, got {}", r5);

    // With higher stability (S=10): R = e^(-5/10) = e^(-0.5) ≈ 0.606
    let entry_high_s = MemoryEntry {
        stability: 10.0,
        ..entry.clone()
    };
    let r5_high = compute_retention(&entry_high_s, today);
    assert!(r5_high > 0.5, "retention with S=10 should still be high, got {}", r5_high);

    // Recent entry (same day): R = e^(-0/S) = 1.0
    let entry_today = MemoryEntry {
        created: today,
        last_recalled: today,
        stability: 1.0,
        ..entry.clone()
    };
    let r0 = compute_retention(&entry_today, today);
    assert!((r0 - 1.0).abs() < 0.001, "same-day retention should be 1.0, got {}", r0);
}

#[test]
fn test_ebbinghaus_should_archive() {
    use neecoder_tauri_lib::memory::ebbinghaus::{MemoryEntry, MemoryCategory, should_archive};

    let today = chrono::Utc::now().date_naive();

    // Fresh entry → should not archive
    let fresh = MemoryEntry {
        id: "fresh".into(),
        key: None,
        text: "- [Lesson] Fresh".into(),
        created: today,
        last_recalled: today,
        recall_count: 0,
        stability: 1.0,
        section: "Test".into(),
        category: MemoryCategory::Lesson,
        session_id: None,
    };
    assert!(!should_archive(&fresh, today));

    // Very old entry with low stability → should archive
    let stale = MemoryEntry {
        id: "stale".into(),
        key: None,
        text: "- [Lesson] Old".into(),
        created: today - chrono::Duration::days(100),
        last_recalled: today - chrono::Duration::days(100),
        recall_count: 1,
        stability: 1.0,
        section: "Test".into(),
        category: MemoryCategory::Lesson,
        session_id: None,
    };
    assert!(should_archive(&stale, today), "very old entry should archive");
}

// ── Context injection ───────────────────────────────────────────────────────

#[test]
fn test_context_injection_with_data() {
    let dir = temp_dir();
    let manager = neecoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));

    // Empty → empty or minimal
    let ctx0 = manager.inject_memory_context();
    assert!(ctx0.len() < 100, "empty memory should produce minimal context");

    // Populate long-term memory
    manager.append_long_term("Rust", "- [Lesson] Rust ownership model prevents data races").expect("append");
    manager.append_long_term("Python", "- [Pattern] Use context managers for file handling").expect("append");

    // Also add a daily note
    manager.append_note("- [Learning] Explored async Rust patterns").expect("note");

    // Context should now include memory
    let ctx = manager.inject_memory_context();
    assert!(!ctx.is_empty(), "context should not be empty");
    assert!(ctx.contains("Memory") || ctx.contains("Notes") || ctx.contains("Rust"),
        "context should reference memory, got: {}", ctx.chars().take(200).collect::<String>());

    cleanup(&dir);
}

// ── Memory search (BM25 + vector hybrid) ────────────────────────────────────

#[test]
fn test_memory_search_finds_relevant() {
    let dir = temp_dir();
    let manager = neecoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));

    // Populate with distinct topics
    manager.append_long_term("Rust", "- [Lesson] Rust has no null. Use Option<T> instead.").expect("append");
    manager.append_long_term("Python", "- [Pattern] Python uses keyword arguments for readability").expect("append");
    manager.append_long_term("Rust", "- [Decision] Use tokio for async Rust").expect("append");

    // Search for Rust-specific content
    let results = manager.search_memory("Rust async", 5).expect("search rust");
    assert!(!results.is_empty(), "should find Rust results");

    // The tokio entry should be most relevant
    let has_tokio = results.iter().any(|r| r.line_content.contains("tokio"));
    assert!(has_tokio || !results.iter().filter(|r| r.line_content.contains("Rust")).collect::<Vec<_>>().is_empty(),
        "should find Rust-related content");

    // Empty query should not crash
    let _empty = manager.search_memory("", 5).expect("empty search");
    // Empty query may return results or empty — either is acceptable

    cleanup(&dir);
}

// ── Session store expiry ────────────────────────────────────────────────────

#[test]
fn test_session_store_with_expiry_config() {
    let dir = temp_dir();
    let storage = neecoder_tauri_lib::memory::session_store::SessionStorage::new(&dir);

    // Create a session
    storage.create_session("expiry-test", "Test").expect("create");
    let msg = neecoder_tauri_lib::chat::ChatMessage {
        role: neecoder_tauri_lib::chat::Role::User,
        content: "test".into(),
        tool_calls: None,
        images: None,
    };
    storage.save_message("expiry-test", &msg, 1).expect("save");

    // 0-day expiry should never delete
    let deleted0 = storage.cleanup_expired_sessions(0).expect("cleanup0");
    assert_eq!(deleted0, 0, "0 days should delete nothing");

    // 9999-day expiry should keep it
    let deleted9k = storage.cleanup_expired_sessions(9999).expect("cleanup9k");
    assert_eq!(deleted9k, 0, "new sessions should not be expired");

    // Session still exists
    let loaded = storage.load_messages("expiry-test").expect("load");
    assert_eq!(loaded.len(), 1);

    storage.delete_session("expiry-test").expect("delete");
    cleanup(&dir);
}

// ── Memory manager max API calls config ─────────────────────────────────────

#[test]
fn test_memory_manager_never_panics_on_empty_state() {
    let dir = temp_dir();
    let manager = neecoder_tauri_lib::memory::MemoryManager::new(dir.join("nonexistent"));

    // All operations should gracefully handle empty/non-existent state
    assert!(manager.read_long_term().is_ok());
    assert!(manager.read_today_note().is_ok());
    assert!(manager.inject_memory_context().len() < 500);
    assert!(manager.get_all_sessions().is_ok());
    assert!(manager.search_memory("anything", 5).is_ok());

    cleanup(&dir);
}
