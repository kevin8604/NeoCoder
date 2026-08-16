//! Smoke tests — verify that all core modules initialize and basic operations
//! work correctly. These run first in CI and catch catastrophic breakage.

use std::path::PathBuf;

// ── Helpers ─────────────────────────────────────────────────────────────────

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neocoder_smoke_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ── Memory Manager Smoke ────────────────────────────────────────────────────

#[test]
fn smoke_memory_manager_create_and_list_sessions() {
    let dir = temp_dir();
    let manager = neocoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));

    let id = manager.create_session().expect("should create session");
    assert!(!id.is_empty(), "session id should not be empty");
    assert!(id.len() > 10, "session id should be UUID-length");

    let sessions = manager.get_all_sessions().expect("should list sessions");
    assert!(sessions.iter().any(|s| s.id == id), "created session should appear in list");

    manager.delete_session(&id).expect("should delete session");
    let after = manager.get_all_sessions().expect("should list after delete");
    assert!(!after.iter().any(|s| s.id == id), "deleted session should be gone");

    cleanup(&dir);
}

#[test]
fn smoke_memory_add_and_read_messages() {
    let dir = temp_dir();
    let manager = neocoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));
    let id = manager.create_session().expect("create");

    let msg = neocoder_tauri_lib::chat::ChatMessage {
        role: neocoder_tauri_lib::chat::Role::User,
        content: "Hello, agent!".into(),
        tool_calls: None,
        images: None,
    };

    manager.add_message(&id, msg).expect("should add message");

    // Read back
    let window = manager.get_context_window(&id, 100_000).expect("should read");
    assert!(!window.is_empty(), "should have messages");
    assert!(window.iter().any(|m| m.content.contains("Hello")), "should find our message");

    cleanup(&dir);
}

#[test]
fn smoke_memory_long_term_read_write() {
    let dir = temp_dir();
    let manager = neocoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));

    manager.write_long_term("# Test Memory\n\n- [Lesson] Always smoke test").expect("write");
    let content = manager.read_long_term().expect("read");
    assert!(content.contains("[Lesson]"), "should contain lesson");
    assert!(content.contains("smoke test"), "should contain content");

    cleanup(&dir);
}

#[test]
fn smoke_memory_context_injection_not_panicking() {
    let dir = temp_dir();
    let manager = neocoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));

    // Should not panic even with empty memory
    let ctx = manager.inject_memory_context();
    assert!(ctx.is_empty() || ctx.contains("Memory") || ctx.contains("Notes"),
        "context should be empty or contain headers, got: {:?}", ctx.chars().take(100).collect::<String>());

    // Add some memory and verify injection picks it up
    manager.append_long_term("Test", "- [Lesson] Smoke test lesson").expect("append");

    let ctx2 = manager.inject_memory_context();
    assert!(!ctx2.is_empty(), "should have content after adding memory");

    cleanup(&dir);
}

#[test]
fn smoke_memory_search() {
    let dir = temp_dir();
    let manager = neocoder_tauri_lib::memory::MemoryManager::new(dir.join("memory"));

    manager.append_long_term("Searchable", "- [Lesson] Rust memory management tips").expect("append");

    let results = manager.search_memory("Rust", 10).expect("search");
    assert!(!results.is_empty(), "should find Rust-related memory");

    let no_results = manager.search_memory("xyzzy_nonexistent_12345", 10).expect("search");
    assert!(no_results.is_empty(), "should not find nonsense query");

    cleanup(&dir);
}

// ── Loop Detector Smoke ─────────────────────────────────────────────────────

#[test]
fn smoke_loop_detector_default_config() {
    use neocoder_tauri_lib::agent::loop_detector::{LoopDetector, LoopDetectionConfig, LoopVerdict};

    let mut detector = LoopDetector::new(LoopDetectionConfig::default());
    assert_eq!(detector.history_len(), 0);

    // No records → Continue
    assert_eq!(detector.check(), LoopVerdict::Continue);

    // Record a few calls — should not trigger
    for _i in 0..2 {
        detector.record_call("grep", &serde_json::json!({"q": "test"}), "found", true);
    }
    assert_eq!(detector.check(), LoopVerdict::Continue);
}

#[test]
fn smoke_loop_detector_repeat_detection() {
    use neocoder_tauri_lib::agent::loop_detector::{LoopDetector, LoopDetectionConfig, LoopVerdict};

    let mut detector = LoopDetector::new(LoopDetectionConfig::default());

    // 3 identical calls → should trigger warning
    for _ in 0..3 {
        detector.record_call("grep", &serde_json::json!({"q": "x"}), "not found", false);
    }
    assert!(matches!(detector.check(), LoopVerdict::InjectWarning(_)));

    // After warning, more of the same → HardStop
    for _ in 0..3 {
        detector.record_call("grep", &serde_json::json!({"q": "x"}), "not found", false);
    }
    assert!(matches!(detector.check(), LoopVerdict::HardStop(_)));
}

#[test]
fn smoke_loop_detector_disabled() {
    use neocoder_tauri_lib::agent::loop_detector::{LoopDetector, LoopDetectionConfig, LoopVerdict};

    let mut detector = LoopDetector::new(LoopDetectionConfig {
        no_progress_threshold: 0,
        ping_pong_cycles: 0,
        failure_streak_threshold: 0,
        read_only_streak_threshold: 0,
        repeated_read_threshold: 0,
    });

    for _ in 0..20 {
        detector.record_call("grep", &serde_json::json!({"q": "x"}), "same", false);
    }
    assert_eq!(detector.check(), LoopVerdict::Continue);
}

// ── Session Storage Smoke ────────────────────────────────────────────────────

#[test]
fn smoke_session_store_create_load_delete() {
    let dir = temp_dir();
    let storage = neocoder_tauri_lib::memory::session_store::SessionStorage::new(&dir);

    let id = "test-session-smoke";
    storage.create_session(id, "Smoke Test").expect("create");

    let msg = neocoder_tauri_lib::chat::ChatMessage {
        role: neocoder_tauri_lib::chat::Role::User,
        content: "smoke message".into(),
        tool_calls: None,
        images: None,
    };
    storage.save_message(id, &msg, 1).expect("save");

    let loaded = storage.load_messages(id).expect("load");
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].content.contains("smoke message"), "expected content to contain 'smoke message', got: {:?}", loaded[0].content);

    // Load with token window
    let window = storage.load_context_window(id, 10).expect("context window");
    assert!(!window.is_empty());

    storage.delete_session(id).expect("delete");

    // Verify deletion
    assert!(storage.load_messages(id).unwrap_or_default().is_empty());

    cleanup(&dir);
}

#[test]
fn smoke_session_store_cleanup_expired_zero_days_noop() {
    let dir = temp_dir();
    let storage = neocoder_tauri_lib::memory::session_store::SessionStorage::new(&dir);

    storage.create_session("keep-me", "Test").expect("create");
    let deleted = storage.cleanup_expired_sessions(0).expect("cleanup");
    assert_eq!(deleted, 0, "0 days should never delete");

    cleanup(&dir);
}

// ── Ebbinghaus Smoke ────────────────────────────────────────────────────────

#[test]
fn smoke_ebbinghaus_retention_compute() {
    use neocoder_tauri_lib::memory::ebbinghaus::{MemoryEntry, MemoryCategory};

    let today = chrono::Utc::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);

    let entry = MemoryEntry {
        id: "smoke-1".into(),
        key: None,
        text: "- [Lesson] Test".into(),
        created: yesterday,
        last_recalled: yesterday,
        recall_count: 1,
        stability: 2.0,
        section: "Test".into(),
        category: MemoryCategory::Lesson,
        session_id: None,
    };

    let retention = neocoder_tauri_lib::memory::ebbinghaus::compute_retention(&entry, today);
    // After 1 day with S=2.0: R = e^(-1/2) ≈ 0.606
    assert!(retention > 0.5 && retention < 0.8,
        "retention should be ~0.606, got {}", retention);
}

#[test]
fn smoke_memory_category_detect_from_text() {
    use neocoder_tauri_lib::memory::ebbinghaus::MemoryCategory;

    assert_eq!(
        MemoryCategory::detect_from_text("- [Lesson] Don't use unwrap in prod"),
        MemoryCategory::Lesson
    );
    assert_eq!(
        MemoryCategory::detect_from_text("- [Decision] Use PostgreSQL"),
        MemoryCategory::Decision
    );
    assert_eq!(
        MemoryCategory::detect_from_text("- [Pattern] Use RAII for resources"),
        MemoryCategory::Pattern
    );
    assert_eq!(
        MemoryCategory::detect_from_text("Just a regular note"),
        MemoryCategory::Coding // default
    );
}

// ── Config Smoke ────────────────────────────────────────────────────────────

#[test]
fn smoke_config_default_values() {
    let settings = neocoder_tauri_lib::config::AppSettings::default();

    assert_eq!(settings.loop_no_progress_threshold, 3);
    assert_eq!(settings.loop_ping_pong_cycles, 2);
    assert_eq!(settings.loop_failure_streak_threshold, 3);
    assert_eq!(settings.session_expiry_days, 0);
    assert_eq!(settings.max_api_calls_per_session, 200);
}

// ── Agent Phase Filtering Smoke ─────────────────────────────────────────────

#[test]
fn smoke_execution_phase_transitions() {
    use neocoder_tauri_lib::agent::ExecutionPhase;

    let planning = ExecutionPhase::Planning;
    let executing = ExecutionPhase::Executing;
    let done = ExecutionPhase::Done;

    // Verify phases are distinct
    assert_ne!(planning, executing);
    assert_ne!(executing, done);
    assert_ne!(planning, done);

    // Verify clone/copy
    let copy = planning;
    assert_eq!(copy, ExecutionPhase::Planning);
}
