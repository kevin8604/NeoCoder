use crate::memory::MemoryManager;
use crate::memory::ebbinghaus::{self, MemoryCategory, MemoryEntry};
use crate::chat::{ChatMessage, Role};
use std::path::PathBuf;
use chrono::NaiveDate;

/// Create a temporary directory for test data (no tempfile crate needed)
fn temp_test_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "neecoder_test_{}_{}", prefix, std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ── Session tests ──

#[test]
fn test_create_session_returns_uuid() {
    let dir = temp_test_dir("create_session");
    let mgr = MemoryManager::new(dir.clone());
    let id = mgr.create_session().unwrap();
    // UUID format: 36 chars with hyphens
    assert_eq!(id.len(), 36);
    assert!(id.contains('-'));
    cleanup(&dir);
}

#[test]
fn test_add_and_get_messages() {
    let dir = temp_test_dir("add_msg");
    let mgr = MemoryManager::new(dir.clone());
    let sid = mgr.create_session().unwrap();

    let user_msg = ChatMessage {
        role: Role::User,
        content: "Hello, NeeCoder!".to_string(),
        tool_calls: None,
        images: None,
    };
    let assistant_msg = ChatMessage {
        role: Role::Assistant,
        content: "Hi there! How can I help?".to_string(),
        tool_calls: None,
        images: None,
    };

    mgr.add_message(&sid, user_msg).unwrap();
    mgr.add_message(&sid, assistant_msg).unwrap();

    let msgs = mgr.get_context_window(&sid, 48000).unwrap();
    assert_eq!(msgs.len(), 2);
    assert!(matches!(msgs[0].role, Role::User));
    assert!(msgs[0].content.contains("Hello"));
    assert!(matches!(msgs[1].role, Role::Assistant));
    cleanup(&dir);
}

#[test]
fn test_get_all_sessions() {
    let dir = temp_test_dir("all_sessions");
    let mgr = MemoryManager::new(dir.clone());

    let _s1 = mgr.create_session().unwrap();
    let _s2 = mgr.create_session().unwrap();

    let sessions = mgr.get_all_sessions().unwrap();
    assert!(sessions.len() >= 2);
    cleanup(&dir);
}

#[test]
fn test_clear_session() {
    let dir = temp_test_dir("clear_session");
    let mgr = MemoryManager::new(dir.clone());
    let sid = mgr.create_session().unwrap();

    mgr.add_message(&sid, ChatMessage {
        role: Role::User,
        content: "Test message".to_string(),
        tool_calls: None,
        images: None,
    }).unwrap();

    let msgs_before = mgr.get_context_window(&sid, 48000).unwrap();
    assert!(!msgs_before.is_empty());

    mgr.clear_session(&sid).unwrap();

    let msgs_after = mgr.get_context_window(&sid, 48000).unwrap();
    assert!(msgs_after.is_empty());
    cleanup(&dir);
}

#[test]
fn test_delete_session() {
    let dir = temp_test_dir("delete_session");
    let mgr = MemoryManager::new(dir.clone());
    let sid = mgr.create_session().unwrap();

    let sessions_before = mgr.get_all_sessions().unwrap().len();
    mgr.delete_session(&sid).unwrap();
    let sessions_after = mgr.get_all_sessions().unwrap().len();

    assert_eq!(sessions_after, sessions_before - 1);
    cleanup(&dir);
}

// ── Long-term memory tests ──

#[test]
fn test_long_term_read_write() {
    let dir = temp_test_dir("longterm");
    let mgr = MemoryManager::new(dir.clone());

    // Initially empty
    let content = mgr.read_long_term().unwrap();
    assert!(content.is_empty());

    // Write content
    mgr.write_long_term("# Important Patterns\n- Use Result for error handling").unwrap();

    let content = mgr.read_long_term().unwrap();
    assert!(content.contains("Important Patterns"));
    cleanup(&dir);
}

#[test]
fn test_long_term_append() {
    let dir = temp_test_dir("longterm_append");
    let mgr = MemoryManager::new(dir.clone());

    mgr.append_long_term("Lessons", "Always handle errors").unwrap();
    mgr.append_long_term("Lessons", "Prefer Result over panic").unwrap();

    let content = mgr.read_long_term().unwrap();
    assert!(content.contains("Always handle errors"));
    assert!(content.contains("Prefer Result over panic"));
    cleanup(&dir);
}

// ── Daily notes tests ──

#[test]
fn test_notes_append_and_read() {
    let dir = temp_test_dir("notes");
    let mgr = MemoryManager::new(dir.clone());

    mgr.append_note("Session completed: fixed 3 bugs").unwrap();
    let today = mgr.read_today_note().unwrap();
    assert!(today.contains("Session completed"));
    cleanup(&dir);
}

// ── Memory context injection ──

#[test]
fn test_inject_memory_context_empty() {
    let dir = temp_test_dir("inject_empty");
    let mgr = MemoryManager::new(dir.clone());

    let ctx = mgr.inject_memory_context();
    // With no data, context should be empty
    assert!(ctx.is_empty());
    cleanup(&dir);
}

#[test]
fn test_inject_memory_context_with_data() {
    let dir = temp_test_dir("inject_data");
    let mgr = MemoryManager::new(dir.clone());

    // Use append to write an entry in parseable format ("- " bullet + section header).
    // The [Decision] tag gives it coding weight so it passes the injection filter.
    mgr.append_long_term("Decisions", "[Decision] Key decisions: use Tauri v2")
        .unwrap();
    mgr.append_note("Today worked on search").unwrap();

    let ctx = mgr.inject_memory_context();
    assert!(ctx.contains("Long-term Memory") || ctx.contains("MEMORY.md"));
    assert!(ctx.contains("Key decisions"));
    cleanup(&dir);
}

// ── Search ──

#[test]
fn test_search_memory_no_results() {
    let dir = temp_test_dir("search_empty");
    let mgr = MemoryManager::new(dir.clone());

    let results = mgr.search_memory("nonexistent topic", 5).unwrap();
    assert!(results.is_empty());
    cleanup(&dir);
}

// ── Edge cases ──

#[test]
fn test_get_context_window_nonexistent_session() {
    let dir = temp_test_dir("nonexist_session");
    let mgr = MemoryManager::new(dir.clone());

    let msgs = mgr.get_context_window("nonexistent-session-id", 48000);
    // Should either return empty or error gracefully
    match msgs {
        Ok(m) => assert!(m.is_empty()),
        Err(_) => {} // error is acceptable for nonexistent session
    }
    cleanup(&dir);
}

#[test]
fn test_create_multiple_sessions() {
    let dir = temp_test_dir("multi_session");
    let mgr = MemoryManager::new(dir.clone());

    let ids: Vec<String> = (0..5).map(|_| mgr.create_session().unwrap()).collect();

    // All IDs should be unique
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 5);
    cleanup(&dir);
}

// ═══════════════════════════════════════════════════════════
// Ebbinghaus Forgetting Curve — Integration Tests
// ═══════════════════════════════════════════════════════════

// ── long_term.read_entries() ──

#[test]
fn test_long_term_read_entries_empty() {
    let dir = temp_test_dir("eb_read_empty");
    let mgr = MemoryManager::new(dir.clone());
    let entries = mgr.long_term.read_entries().unwrap();
    assert!(entries.is_empty(), "Empty MEMORY.md should yield no entries");
    cleanup(&dir);
}

#[test]
fn test_long_term_read_entries_with_metadata() {
    let dir = temp_test_dir("eb_read_meta");
    let mgr = MemoryManager::new(dir.clone());

    // Write MEMORY.md with explicit metadata
    let content = r#"
## Learned Patterns

- [Lesson] Use small learning rate for LoRA
<!-- mem: created=2026-06-01 recalled=2026-06-20 count=5 S=3.50 -->
- [Decision] Prefer Tauri v2 over Electron
<!-- mem: created=2026-05-15 recalled=2026-06-25 count=10 S=8.20 -->
"#;
    mgr.long_term.write(content).unwrap();

    let entries = mgr.long_term.read_entries().unwrap();
    assert_eq!(entries.len(), 2, "Should parse 2 entries");

    assert_eq!(entries[0].recall_count, 5);
    assert!((entries[0].stability - 3.5).abs() < 0.01);
    assert_eq!(entries[0].section, "Learned Patterns");
    assert!(entries[0].text.contains("LoRA"));

    assert_eq!(entries[1].recall_count, 10);
    assert!((entries[1].stability - 8.2).abs() < 0.01);
    assert!(entries[1].text.contains("Tauri"));

    cleanup(&dir);
}

#[test]
fn test_long_term_read_entries_backward_compat() {
    let dir = temp_test_dir("eb_read_compat");
    let mgr = MemoryManager::new(dir.clone());

    // Write old-format MEMORY.md (no metadata)
    let content = "## Lessons\n\n- Always handle errors\n- Prefer Result over panic\n";
    mgr.long_term.write(content).unwrap();

    let entries = mgr.long_term.read_entries().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].recall_count, 0, "No metadata → count=0");
    assert_eq!(entries[0].stability, 1.0, "No metadata → S=1.0");
    assert_eq!(entries[1].recall_count, 0);
    cleanup(&dir);
}

// ── long_term.append() with Ebbinghaus metadata ──

#[test]
fn test_long_term_append_creates_metadata() {
    let dir = temp_test_dir("eb_append_meta");
    let mgr = MemoryManager::new(dir.clone());

    mgr.append_long_term("Patterns", "Use async/await for IO").unwrap();

    let entries = mgr.long_term.read_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].text.contains("async/await"));
    assert_eq!(entries[0].section, "Patterns");
    assert_eq!(entries[0].recall_count, 0);
    assert_eq!(entries[0].stability, 1.0);
    cleanup(&dir);
}

// ── long_term.write_entries() / read_entries() roundtrip ──

#[test]
fn test_long_term_write_read_roundtrip() {
    let dir = temp_test_dir("eb_roundtrip");
    let mgr = MemoryManager::new(dir.clone());

    let entries = vec![
        MemoryEntry {
            id: "test-1".to_string(),
            key: None,
            text: "- [Lesson] Always close DB connections".to_string(),
            created: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            last_recalled: NaiveDate::from_ymd_opt(2026, 6, 20).unwrap(),
            recall_count: 3,
            stability: 2.5,
            section: "Patterns".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
        MemoryEntry {
            id: "test-2".to_string(),
            key: None,
            text: "- [Decision] Use PostgreSQL over SQLite".to_string(),
            created: NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
            last_recalled: NaiveDate::from_ymd_opt(2026, 6, 25).unwrap(),
            recall_count: 8,
            stability: 6.0,
            section: "Architecture".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
    ];

    mgr.long_term.write_entries(&entries).unwrap();
    let parsed = mgr.long_term.read_entries().unwrap();

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].recall_count, 3);
    assert!((parsed[0].stability - 2.5).abs() < 0.01);
    assert_eq!(parsed[0].section, "Patterns");
    assert_eq!(parsed[1].recall_count, 8);
    assert!((parsed[1].stability - 6.0).abs() < 0.01);
    assert_eq!(parsed[1].section, "Architecture");
    cleanup(&dir);
}

// ── long_term.recall_entries() ──

#[test]
fn test_long_term_recall_updates_metadata() {
    let dir = temp_test_dir("eb_recall");
    let mgr = MemoryManager::new(dir.clone());

    let entry = MemoryEntry {
        id: "test-recall".to_string(),
        key: None,
        text: "- [Lesson] Test recall".to_string(),
        created: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        last_recalled: NaiveDate::from_ymd_opt(2026, 6, 10).unwrap(),
        recall_count: 2,
        stability: 2.0,
        section: "Test".to_string(),
        category: MemoryCategory::Coding,
        session_id: None,
    };
    let original_s = entry.stability;

    mgr.long_term.write_entries(&[entry.clone()]).unwrap();
    mgr.long_term.recall_entries(&[0]).unwrap();

    let updated = mgr.long_term.read_entries().unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].recall_count, 3, "Count should be 2+1=3");
    assert!(updated[0].stability > original_s, "S should increase after recall");
    // S += ln(更新后 count+1) = ln(4) ≈ 1.39
    let expected_s = original_s + 4.0_f64.ln();
    assert!((updated[0].stability - expected_s).abs() < 0.01);
    cleanup(&dir);
}

#[test]
fn test_long_term_recall_multiple_indices() {
    let dir = temp_test_dir("eb_recall_multi");
    let mgr = MemoryManager::new(dir.clone());

    let entries = vec![
        MemoryEntry::new("- Entry A".to_string(), "S1".to_string()),
        MemoryEntry::new("- Entry B".to_string(), "S1".to_string()),
        MemoryEntry::new("- Entry C".to_string(), "S1".to_string()),
    ];
    mgr.long_term.write_entries(&entries).unwrap();

    // Recall entries 0 and 2, skip 1
    mgr.long_term.recall_entries(&[0, 2]).unwrap();

    let updated = mgr.long_term.read_entries().unwrap();
    assert_eq!(updated[0].recall_count, 1, "Entry A should be recalled");
    assert_eq!(updated[1].recall_count, 0, "Entry B should NOT be recalled");
    assert_eq!(updated[2].recall_count, 1, "Entry C should be recalled");
    cleanup(&dir);
}

#[test]
fn test_long_term_recall_out_of_bounds() {
    let dir = temp_test_dir("eb_recall_oob");
    let mgr = MemoryManager::new(dir.clone());

    let entries = vec![MemoryEntry::new("- Only entry".to_string(), "S".to_string())];
    mgr.long_term.write_entries(&entries).unwrap();

    // Index 99 doesn't exist — should not panic
    mgr.long_term.recall_entries(&[0, 99]).unwrap();
    let updated = mgr.long_term.read_entries().unwrap();
    assert_eq!(updated[0].recall_count, 1);
    cleanup(&dir);
}

// ── long_term.cleanup_expired() ──

#[test]
fn test_long_term_cleanup_removes_expired() {
    let dir = temp_test_dir("eb_cleanup");
    let mgr = MemoryManager::new(dir.clone());

    // 相对当前日期构造条目：避免硬编码日期随时间漂移导致判定翻转
    let now = chrono::Utc::now().date_naive();

    let entries = vec![
        // Fresh entry — should survive
        MemoryEntry {
            id: "fresh".to_string(),
            key: None,
            text: "- Fresh entry".to_string(),
            created: now - chrono::Duration::days(6),
            last_recalled: now - chrono::Duration::days(5),
            recall_count: 5,
            stability: 10.0,
            section: "Active".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
        // Ancient entry with low S — should be archived
        MemoryEntry {
            id: "forgotten".to_string(),
            key: None,
            text: "- Forgotten entry".to_string(),
            created: now - chrono::Duration::days(200),
            last_recalled: now - chrono::Duration::days(120),
            recall_count: 0,
            stability: 1.0,
            section: "Old".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
    ];
    mgr.long_term.write_entries(&entries).unwrap();

    let archived = mgr.long_term.cleanup_expired().unwrap();
    assert_eq!(archived, 1, "Should archive 1 expired entry");

    let remaining = mgr.long_term.read_entries().unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].text.contains("Fresh entry"));
    cleanup(&dir);
}

#[test]
fn test_long_term_cleanup_no_expired() {
    let dir = temp_test_dir("eb_cleanup_none");
    let mgr = MemoryManager::new(dir.clone());

    // All fresh entries
    let entries = vec![
        MemoryEntry {
            id: "entry-a".to_string(),
            key: None,
            text: "- Entry A".to_string(),
            created: NaiveDate::from_ymd_opt(2026, 6, 25).unwrap(),
            last_recalled: NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
            recall_count: 10,
            stability: 50.0,
            section: "S".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
    ];
    mgr.long_term.write_entries(&entries).unwrap();

    let archived = mgr.long_term.cleanup_expired().unwrap();
    assert_eq!(archived, 0, "Nothing to archive");

    let remaining = mgr.long_term.read_entries().unwrap();
    assert_eq!(remaining.len(), 1);
    cleanup(&dir);
}

#[test]
fn test_long_term_cleanup_recent_but_low_r() {
    let dir = temp_test_dir("eb_cleanup_recent");
    let mgr = MemoryManager::new(dir.clone());

    // 相对当前日期构造条目：低 R 但最近（30 天内）被回忆过——不应归档
    let now = chrono::Utc::now().date_naive();

    // Low R but recalled recently (within 30 days) — should NOT archive
    let entries = vec![
        MemoryEntry {
            id: "recent-low".to_string(),
            key: None,
            text: "- Recent low-R entry".to_string(),
            created: now - chrono::Duration::days(6),
            last_recalled: now - chrono::Duration::days(5),
            recall_count: 0,
            stability: 1.0, // R after 5 days ≈ 0.0067 but < 30 days
            section: "S".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
    ];
    mgr.long_term.write_entries(&entries).unwrap();

    let archived = mgr.long_term.cleanup_expired().unwrap();
    assert_eq!(archived, 0, "Recent entry should not be archived even with low R");

    let remaining = mgr.long_term.read_entries().unwrap();
    assert_eq!(remaining.len(), 1);
    cleanup(&dir);
}

// ── inject_memory_context() with Ebbinghaus ──

#[test]
fn test_inject_context_r_value_sorting() {
    let dir = temp_test_dir("eb_inject_sort");
    let mgr = MemoryManager::new(dir.clone());

    // 相对当前日期构造条目：避免硬编码日期随时间漂移导致判定翻转
    let now = chrono::Utc::now().date_naive();

    // Create entries with different retention values
    let entries = vec![
        // Low R: recalled 20 days ago, S=1.0
        MemoryEntry {
            id: "low-r".to_string(),
            key: None,
            text: "- Low retention entry".to_string(),
            created: now - chrono::Duration::days(30),
            last_recalled: now - chrono::Duration::days(20),
            recall_count: 0,
            stability: 1.0,
            section: "Test".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
        // High R: recalled yesterday, S=10.0
        MemoryEntry {
            id: "high-r".to_string(),
            key: None,
            text: "- High retention entry".to_string(),
            created: now - chrono::Duration::days(30),
            last_recalled: now - chrono::Duration::days(1),
            recall_count: 15,
            stability: 10.0,
            section: "Test".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
    ];
    mgr.long_term.write_entries(&entries).unwrap();

    let ctx = mgr.inject_memory_context();

    // Both entries should be present (both have R > 0.01)
    assert!(ctx.contains("High retention entry"));
    // Low retention entry has R = e^(-20) ≈ 0.000000002, below 0.01 threshold → filtered out
    assert!(!ctx.contains("Low retention entry"), "Very low R entries should be filtered");

    // High R entry should show R value annotation
    assert!(ctx.contains("R="), "Context should show R-value annotation");

    cleanup(&dir);
}

#[test]
fn test_inject_context_triggers_recall() {
    let dir = temp_test_dir("eb_inject_recall");
    let mgr = MemoryManager::new(dir.clone());

    // 相对当前日期构造条目：最近回忆过 → R 高 → 通过注入过滤并触发 recall
    let now = chrono::Utc::now().date_naive();

    let entry = MemoryEntry {
        id: "recall-me".to_string(),
        key: None,
        text: "- Will be recalled".to_string(),
        created: now - chrono::Duration::days(2),
        last_recalled: now - chrono::Duration::days(1),
        recall_count: 0,
        stability: 1.0,
        section: "Test".to_string(),
        category: MemoryCategory::Coding,
        session_id: None,
    };
    mgr.long_term.write_entries(&[entry]).unwrap();

    // inject_memory_context should trigger recall
    let _ctx = mgr.inject_memory_context();

    let updated = mgr.long_term.read_entries().unwrap();
    assert_eq!(updated.len(), 1);
    assert!(updated[0].recall_count >= 1, "Injection should trigger recall");
    assert!(updated[0].stability > 1.0, "S should increase after injection recall");
    cleanup(&dir);
}

#[test]
fn test_inject_context_section_grouping() {
    let dir = temp_test_dir("eb_inject_sections");
    let mgr = MemoryManager::new(dir.clone());

    // 相对当前日期构造条目：最近回忆过 → R 高 → 通过注入过滤
    let now = chrono::Utc::now().date_naive();

    let entries = vec![
        MemoryEntry {
            id: "pattern-a".to_string(),
            key: None,
            text: "- Pattern A".to_string(),
            created: now - chrono::Duration::days(2),
            last_recalled: now - chrono::Duration::days(1),
            recall_count: 5,
            stability: 5.0,
            section: "Patterns".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
        MemoryEntry {
            id: "decision-b".to_string(),
            key: None,
            text: "- Decision B".to_string(),
            created: now - chrono::Duration::days(2),
            last_recalled: now - chrono::Duration::days(1),
            recall_count: 3,
            stability: 3.0,
            section: "Decisions".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        },
    ];
    mgr.long_term.write_entries(&entries).unwrap();

    let ctx = mgr.inject_memory_context();
    assert!(ctx.contains("### Patterns"), "Should contain Patterns section header");
    assert!(ctx.contains("### Decisions"), "Should contain Decisions section header");
    cleanup(&dir);
}

#[test]
fn test_inject_context_max_entries_limit() {
    let dir = temp_test_dir("eb_inject_limit");
    let mgr = MemoryManager::new(dir.clone());

    // Create 25 entries (more than MAX_LT_ENTRIES=20)
    let entries: Vec<MemoryEntry> = (0..25)
        .map(|i| MemoryEntry {
            id: format!("entry-{}", i),
            key: None,
            text: format!("- Entry {}", i),
            created: NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
            last_recalled: NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(),
            recall_count: i as u32,
            stability: 5.0 + i as f64,
            section: "Bulk".to_string(),
            category: MemoryCategory::Coding,
            session_id: None,
        })
        .collect();
    mgr.long_term.write_entries(&entries).unwrap();

    let ctx = mgr.inject_memory_context();

    // Count how many entries appear in the context
    let entry_count = ctx.lines()
        .filter(|l| l.starts_with("- Entry "))
        .count();
    assert!(entry_count <= 20, "Should inject at most 20 entries, got {}", entry_count);
    cleanup(&dir);
}

// ── search_memory() with Ebbinghaus recall ──

#[test]
fn test_search_triggers_recall() {
    let dir = temp_test_dir("eb_search_recall");
    let mgr = MemoryManager::new(dir.clone());

    // Write an entry with known text
    let entry = MemoryEntry {
        id: "search-recall".to_string(),
        key: None,
        text: "- [Lesson] Always use Result for error handling".to_string(),
        created: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        last_recalled: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        recall_count: 0,
        stability: 1.0,
        section: "Patterns".to_string(),
        category: MemoryCategory::Coding,
        session_id: None,
    };
    mgr.long_term.write_entries(&[entry]).unwrap();

    // Search for the entry
    let results = mgr.search_memory("Result", 5).unwrap();
    assert!(!results.is_empty(), "Should find the entry");

    // Verify recall was triggered
    let updated = mgr.long_term.read_entries().unwrap();
    assert_eq!(updated.len(), 1);
    assert!(updated[0].recall_count >= 1, "Search hit should trigger recall");
    cleanup(&dir);
}

#[test]
fn test_search_no_match_no_recall() {
    let dir = temp_test_dir("eb_search_no");
    let mgr = MemoryManager::new(dir.clone());

    let entry = MemoryEntry {
        id: "no-recall".to_string(),
        key: None,
        text: "- [Lesson] Use async/await".to_string(),
        created: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        last_recalled: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        recall_count: 0,
        stability: 1.0,
        section: "Patterns".to_string(),
        category: MemoryCategory::Coding,
        session_id: None,
    };
    mgr.long_term.write_entries(&[entry]).unwrap();

    // Search for something that doesn't match
    let results = mgr.search_memory("Kubernetes", 5).unwrap();
    assert!(results.is_empty(), "Should find no results");

    // Verify no recall happened
    let updated = mgr.long_term.read_entries().unwrap();
    assert_eq!(updated[0].recall_count, 0, "No match → no recall");
    cleanup(&dir);
}

// ── Ebbinghaus formula edge cases ──

#[test]
fn test_ebbinghaus_retention_zero_stability() {
    let mut entry = MemoryEntry::new("- test".to_string(), "Test".to_string());
    entry.stability = 0.0;
    let now = chrono::Utc::now().date_naive();
    let r = ebbinghaus::compute_retention(&entry, now);
    assert_eq!(r, 0.0, "Zero stability → R=0");
}

#[test]
fn test_ebbinghaus_retention_negative_stability() {
    let mut entry = MemoryEntry::new("- test".to_string(), "Test".to_string());
    entry.stability = -1.0;
    let now = chrono::Utc::now().date_naive();
    let r = ebbinghaus::compute_retention(&entry, now);
    assert_eq!(r, 0.0, "Negative stability → R=0");
}

#[test]
fn test_ebbinghaus_multiple_recalls_increase_s() {
    let mut entry = MemoryEntry::new("- test".to_string(), "Test".to_string());
    let s_values: Vec<f64> = (0..5)
        .map(|_| {
            ebbinghaus::update_recall(&mut entry);
            entry.stability
        })
        .collect();

    // Each S should be strictly greater than the previous
    for i in 1..s_values.len() {
        assert!(s_values[i] > s_values[i - 1], "S should monotonically increase");
    }

    // Growth should show diminishing returns
    let growth_1 = s_values[1] - s_values[0]; // ln(3)
    let growth_4 = s_values[4] - s_values[3]; // ln(6)
    assert!(growth_4 > growth_1, "Growth should increase with count (ln grows)");
}

#[test]
fn test_ebbinghaus_archive_boundary() {
    let mut entry = MemoryEntry::new("- boundary".to_string(), "Test".to_string());
    let now = NaiveDate::from_ymd_opt(2026, 6, 26).unwrap();

    // Exactly 30 days ago — should NOT archive (boundary: > 30, not >=)
    entry.last_recalled = NaiveDate::from_ymd_opt(2026, 5, 27).unwrap();
    assert!(!ebbinghaus::should_archive(&entry, now), "30 days exactly should not archive");

    // 31 days ago with very low S — should archive
    entry.last_recalled = NaiveDate::from_ymd_opt(2026, 5, 26).unwrap();
    assert!(ebbinghaus::should_archive(&entry, now), "31 days + low S should archive");
}

#[test]
fn test_parse_metadata_malformed() {
    // Missing fields
    assert!(ebbinghaus::parse_metadata("<!-- mem: created=2026-06-01 -->").is_none());
    // Wrong format
    assert!(ebbinghaus::parse_metadata("not a metadata line").is_none());
    // Empty
    assert!(ebbinghaus::parse_metadata("").is_none());
    // Partial fields
    assert!(ebbinghaus::parse_metadata("<!-- mem: created=2026-06-01 count=5 -->").is_none());
}

#[test]
fn test_parse_entries_multi_section() {
    let content = r#"## Section A

- Entry A1
<!-- mem: created=2026-06-01 recalled=2026-06-20 count=1 S=1.50 -->
- Entry A2
<!-- mem: created=2026-06-02 recalled=2026-06-21 count=2 S=2.00 -->

## Section B

- Entry B1
<!-- mem: created=2026-06-03 recalled=2026-06-22 count=3 S=3.00 -->
"#;
    let entries = ebbinghaus::parse_memory_entries(content);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].section, "Section A");
    assert_eq!(entries[1].section, "Section A");
    assert_eq!(entries[2].section, "Section B");
    assert_eq!(entries[2].recall_count, 3);
}

#[test]
fn test_parse_entries_empty_content() {
    let entries = ebbinghaus::parse_memory_entries("");
    assert!(entries.is_empty());
}

#[test]
fn test_parse_entries_no_entries() {
    let content = "# Header\n\nSome text without entries.\n\n## Section\n\nJust text.\n";
    let entries = ebbinghaus::parse_memory_entries(content);
    assert!(entries.is_empty());
}
