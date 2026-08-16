//! Loop Detection system for the Agent harness.
//!
//! Detects four types of non-progress patterns in tool call sequences
//! and applies a two-level verdict: InjectWarning → HardStop.
//!
//! ## Detection Strategies
//!
//! 1. **No-Progress Repeat**: Same (tool_name + args + output_hash) N times
//! 2. **Ping-Pong**: Two tools alternating A→B→A→B, N cycles
//! 3. **Consecutive Failure Streak**: Same tool fails N times in a row
//! 4. **Read-Only Streak**: Deduped by (tool_name + args_sig) — only repeated reads
//!    of the same target count as a loop; reading different files is valid exploration.
//!
//! ## Verdict State Machine
//!
//! ```text
//! Continue ──(detected)──> InjectWarning ──(detected again)──> HardStop
//! ```

use std::collections::{HashMap, HashSet, VecDeque};

/// Tools considered read-only (no side-effects on the codebase).
const READ_ONLY_TOOLS: &[&str] = &[
    "read_file",
    "glob",
    "grep",
    "search_codebase",
    "list_directory",
    "get_symbols",
    "get_diagnostics",
    "memory_search",
    "git_status",
    "git_diff",
    "web_search",
    "web_fetch",
];

/// Returns true if the tool name is a read-only tool.
fn is_read_only_tool(name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&name)
}

// ── Config ──────────────────────────────────────────────────────────────────

/// Configuration for loop detection, with 0 = disabled for each strategy.
#[derive(Debug, Clone)]
pub struct LoopDetectionConfig {
    /// Number of identical (name + args + output) calls before triggering.
    /// Set to 0 to disable this strategy.
    pub no_progress_threshold: usize,
    /// Number of A→B→A→B cycles before triggering.
    /// Set to 0 to disable this strategy.
    pub ping_pong_cycles: usize,
    /// Number of consecutive failures for the same tool before triggering.
    /// Set to 0 to disable this strategy.
    pub failure_streak_threshold: usize,
    /// Number of consecutive read-only tool calls (total, before dedup) to even
    /// consider this strategy. Set to 0 to disable.
    pub read_only_streak_threshold: usize,
    /// Max consecutive times the same (tool_name + args_sig + output) can appear
    /// in a read-only streak before triggering. Changing output (e.g. polling a
    /// build log) resets the repeat count. Set to 0 to disable repeated-read detection.
    pub repeated_read_threshold: usize,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        Self {
            no_progress_threshold: 5,
            ping_pong_cycles: 3,
            failure_streak_threshold: 5,
            read_only_streak_threshold: 15,
            repeated_read_threshold: 3,
        }
    }
}

// ── Call Record ─────────────────────────────────────────────────────────────

/// An immutable record of a single tool call outcome.
#[derive(Debug, Clone)]
struct CallRecord {
    tool_name: String,
    /// Normalized JSON string of the call arguments.
    args_sig: String,
    /// Hash of the first 4096 bytes of the output (UTF-8 safe).
    result_hash: u64,
    /// Whether the tool call succeeded (reserved for future use).
    #[allow(dead_code)]
    success: bool,
}

// ── Verdict ─────────────────────────────────────────────────────────────────

/// Verdict produced by the loop detector after each check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopVerdict {
    /// No loop detected — continue normally.
    Continue,
    /// First-time detection — inject a self-correction hint into context.
    InjectWarning(String),
    /// Warning was already injected but the loop persisted — force termination.
    HardStop(String),
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Hash the first `max_bytes` of a UTF-8 output safely.
/// Uses the floor-UTF-8-char-boundary pattern to avoid panics with CJK text.
fn hash_output(output: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    const HASH_PREFIX_BYTES: usize = 4096;
    let slice = if output.len() > HASH_PREFIX_BYTES {
        // Safe UTF-8 boundary truncation
        let mut end = HASH_PREFIX_BYTES;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        &output[..end]
    } else {
        output
    };
    slice.hash(&mut hasher);
    hasher.finish()
}

/// Normalize tool arguments to a canonical JSON string for comparison.
fn args_signature(args: &serde_json::Value) -> String {
    serde_json::to_string(args).unwrap_or_default()
}

// ── Loop Detector ───────────────────────────────────────────────────────────

pub struct LoopDetector {
    config: LoopDetectionConfig,
    history: VecDeque<CallRecord>,
    /// Per-tool consecutive failure counter (reset on success).
    consecutive_failures: HashMap<String, usize>,
    /// Whether a warning has been injected since last Continue.
    warning_injected: bool,

    // ── Incremental read-only streak state (updated in record_call) ──
    /// Current consecutive read-only call count (reset to 0 on non-read-only).
    read_only_streak: usize,
    /// Consecutive count of the same (tool_name, args_sig, output) at the tail.
    tail_repeat_count: usize,
    /// Unique (tool_name, args_sig) pairs within the current read-only streak.
    read_only_unique_keys: HashSet<(String, String)>,
}

impl LoopDetector {
    pub fn new(config: LoopDetectionConfig) -> Self {
        Self {
            config,
            history: VecDeque::new(),
            consecutive_failures: HashMap::new(),
            warning_injected: false,
            read_only_streak: 0,
            tail_repeat_count: 0,
            read_only_unique_keys: HashSet::new(),
        }
    }

    /// Record a completed tool call for later analysis.
    pub fn record_call(
        &mut self,
        tool_name: &str,
        args: &serde_json::Value,
        output: &str,
        success: bool,
    ) {
        log::debug!(
            "[LoopDetector] record_call: tool='{}', success={}, args_sig_len={}, output_len={}",
            tool_name,
            success,
            args.to_string().len(),
            output.len()
        );

        let record = CallRecord {
            tool_name: tool_name.to_string(),
            args_sig: args_signature(args),
            result_hash: hash_output(output),
            success,
        };

        // Update per-tool failure counter
        if success {
            self.consecutive_failures.remove(tool_name);
        } else {
            *self
                .consecutive_failures
                .entry(tool_name.to_string())
                .or_insert(0) += 1;
        }

        // ── Update incremental read-only streak state ──
        if is_read_only_tool(tool_name) {
            self.read_only_streak += 1;
            self.read_only_unique_keys
                .insert((tool_name.to_string(), record.args_sig.clone()));

            // Track consecutive identical (tool_name, args_sig, output) at the tail.
            // Output changes (e.g. polling a process) reset the count — only truly
            // static re-reads are loops.
            if let Some(prev) = self.history.back() {
                if prev.tool_name == tool_name
                    && prev.args_sig == record.args_sig
                    && prev.result_hash == record.result_hash
                {
                    self.tail_repeat_count += 1;
                } else {
                    self.tail_repeat_count = 1;
                }
            } else {
                self.tail_repeat_count = 1;
            }
        } else {
            // Non-read-only tool打断 streak
            self.read_only_streak = 0;
            self.tail_repeat_count = 0;
            self.read_only_unique_keys.clear();
        }

        self.history.push_back(record);
    }

    /// Run all detection strategies and return a verdict.
    pub fn check(&mut self) -> LoopVerdict {
        // Strategy 1: No-Progress Repeat
        if let Some(reason) = self.check_no_progress_repeat() {
            return self.issue_verdict(reason);
        }

        // Strategy 2: Ping-Pong
        if let Some(reason) = self.check_ping_pong() {
            return self.issue_verdict(reason);
        }

        // Strategy 3: Consecutive Failure Streak
        if let Some(reason) = self.check_failure_streak() {
            return self.issue_verdict(reason);
        }

        // Strategy 4: Read-Only Streak (exploration without action)
        if let Some(reason) = self.check_read_only_streak() {
            return self.issue_verdict(reason);
        }

        // No strategy triggered — reset warning flag so next detection
        // starts from InjectWarning (not immediate HardStop).
        self.warning_injected = false;
        LoopVerdict::Continue
    }

    /// Public accessor for per-tool failure counts (used by enhanced failure detection).
    pub fn per_tool_failures(&self) -> &HashMap<String, usize> {
        &self.consecutive_failures
    }

    /// How many consecutive failures for a specific tool.
    pub fn failure_count(&self, tool_name: &str) -> usize {
        self.consecutive_failures
            .get(tool_name)
            .copied()
            .unwrap_or(0)
    }

    /// Total number of records kept.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Current read-only streak count (for debugging).
    pub fn get_read_only_streak(&self) -> usize {
        self.read_only_streak
    }

    /// Current tail repeat count (for debugging).
    pub fn get_tail_repeat_count(&self) -> usize {
        self.tail_repeat_count
    }

    /// Current number of unique (tool_name, args_sig) in read-only streak (for debugging).
    pub fn get_unique_keys_count(&self) -> usize {
        self.read_only_unique_keys.len()
    }

    // ── Private detection methods ──

    fn check_no_progress_repeat(&self) -> Option<String> {
        let threshold = self.config.no_progress_threshold;
        if threshold == 0 || self.history.len() < threshold {
            return None;
        }

        let last = self.history.back()?;
        let mut count: usize = 1;
        for record in self.history.iter().rev().skip(1) {
            if record.tool_name == last.tool_name
                && record.args_sig == last.args_sig
                && record.result_hash == last.result_hash
            {
                count += 1;
            } else {
                break;
            }
        }

        if count >= threshold {
            Some(format!(
                "Tool '{}' called {} times with identical arguments and produced the same output",
                last.tool_name, count
            ))
        } else {
            None
        }
    }

    fn check_ping_pong(&self) -> Option<String> {
        let cycles = self.config.ping_pong_cycles;
        let needed = cycles * 2;
        if cycles == 0 || self.history.len() < needed {
            return None;
        }

        // Take the last `needed` records
        let recent: Vec<&CallRecord> = {
            let len = self.history.len();
            self.history.iter().skip(len - needed).collect()
        };

        let a = recent[0];
        let b = recent[1];

        // Must be two different tools
        if a.tool_name == b.tool_name {
            return None;
        }

        // Verify the alternating pattern
        for i in 0..cycles {
            let idx_a = i * 2;
            let idx_b = i * 2 + 1;

            if recent[idx_a].tool_name != a.tool_name
                || recent[idx_a].args_sig != a.args_sig
                || recent[idx_a].result_hash != a.result_hash
            {
                return None;
            }
            if recent[idx_b].tool_name != b.tool_name
                || recent[idx_b].args_sig != b.args_sig
                || recent[idx_b].result_hash != b.result_hash
            {
                return None;
            }
        }

        Some(format!(
            "Ping-pong loop: '{}' and '{}' alternating {} times with identical results",
            a.tool_name, b.tool_name, cycles
        ))
    }

    fn check_failure_streak(&self) -> Option<String> {
        let threshold = self.config.failure_streak_threshold;
        if threshold == 0 {
            return None;
        }

        for (tool_name, count) in &self.consecutive_failures {
            if *count >= threshold {
                return Some(format!(
                    "Tool '{}' has failed {} consecutive times",
                    tool_name, count
                ));
            }
        }
        None
    }

    /// Strategy 4: Read-Only Streak with (tool_name, args_sig) dedup.
    ///
    /// Uses incremental state maintained in `record_call` — O(1) check.
    ///
    /// Two sub-rules, either triggers:
    ///
    /// A) **Repeated-read**: the same (tool_name, args_sig, output) appears
    ///    `repeated_read_threshold`+ times consecutively at the tail. Different
    ///    outputs (e.g. polling) reset the repeat count.
    ///
    /// B) **Blind exploration**: total consecutive read-only calls >=
    ///    `read_only_streak_threshold` AND unique (tool_name, args_sig) count <
    ///    50% of total.
    ///
    /// Reading many *different* files (unique >= 50%) is considered valid
    /// exploration and will NOT trigger, regardless of total count.
    fn check_read_only_streak(&self) -> Option<String> {
        let total = self.read_only_streak;
        if total == 0 {
            return None;
        }

        let repeated_threshold = self.config.repeated_read_threshold;
        let total_threshold = self.config.read_only_streak_threshold;

        // Rule A: Tail consecutive identical (tool_name, args_sig)
        if repeated_threshold > 0 && self.tail_repeat_count >= repeated_threshold {
            let last = self.history.back()?;
            return Some(format!(
                "Repeated read: '{}' with identical arguments read {} consecutive times. \
                 The agent is stuck re-reading the same target",
                last.tool_name, self.tail_repeat_count
            ));
        }

        // Rule B: Total streak long enough + significant repetition (unique < 50%)
        if total_threshold > 0 && total >= total_threshold {
            let unique_count = self.read_only_unique_keys.len();
            if unique_count * 2 < total {
                return Some(format!(
                    "Read-only streak: {} consecutive read-only calls with only {} unique targets. \
                     The agent is exploring without making progress",
                    total, unique_count
                ));
            }
        }

        None
    }

    fn issue_verdict(&mut self, reason: String) -> LoopVerdict {
        if self.warning_injected {
            LoopVerdict::HardStop(format!(
                "Loop persisted after warning. {reason}. Terminating to prevent resource waste."
            ))
        } else {
            self.warning_injected = true;
            LoopVerdict::InjectWarning(format!(
                "IMPORTANT: A loop pattern has been detected in your tool usage.\n\
                 {reason}. You must change your approach:\n\
                 (1) Try a different tool or different arguments,\n\
                 (2) If polling a process, increase wait time or check if it's stuck,\n\
                 (3) If the task cannot be completed, explain why and stop.\n\
                 Do NOT repeat the same tool call with the same arguments."
            ))
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_args(s: &str) -> serde_json::Value {
        serde_json::json!({ "query": s })
    }

    #[test]
    fn test_no_progress_repeat_detected() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        for _ in 0..3 {
            detector.record_call("grep", &make_args("hello"), "File not found", false);
        }

        match detector.check() {
            LoopVerdict::InjectWarning(msg) => {
                assert!(msg.contains("grep"), "warning should name the tool");
                assert!(
                    msg.contains("3 consecutive times"),
                    "warning should mention count"
                );
            }
            other => panic!("Expected InjectWarning, got {:?}", other),
        }
    }

    #[test]
    fn test_no_progress_different_output_no_trigger() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        detector.record_call("grep", &make_args("a"), "result1", false);
        detector.record_call("grep", &make_args("a"), "result2", false);
        detector.record_call("grep", &make_args("a"), "result3", false);

        // Different outputs → no trigger (result_hash differs)
        assert_eq!(detector.check(), LoopVerdict::Continue);
    }

    #[test]
    fn test_ping_pong_detected() {
        let mut detector = LoopDetector::new(LoopDetectionConfig {
            ping_pong_cycles: 2,
            ..Default::default()
        });

        // A→B→A→B (2 cycles, 4 calls)
        detector.record_call("read_file", &make_args("a.rs"), "content A", true);
        detector.record_call("edit", &make_args("fix a"), "error", false);
        detector.record_call("read_file", &make_args("a.rs"), "content A", true);
        detector.record_call("edit", &make_args("fix a"), "error", false);

        match detector.check() {
            LoopVerdict::InjectWarning(msg) => {
                assert!(msg.contains("Ping-pong"), "should detect ping-pong");
                assert!(msg.contains("read_file"), "should name tool A");
                assert!(msg.contains("edit"), "should name tool B");
            }
            other => panic!("Expected InjectWarning for ping-pong, got {:?}", other),
        }
    }

    #[test]
    fn test_failure_streak_detected() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        // 每次 args 不同：避免同时命中 no-progress 策略（阈值同为 5 且先检查），
        // 确保验证的是 failure-streak 策略本身
        for i in 0..5 {
            detector.record_call(
                "edit",
                &make_args(&format!("diff a{}", i)),
                "error: mismatched types",
                false,
            );
        }

        match detector.check() {
            LoopVerdict::InjectWarning(msg) => {
                assert!(msg.contains("edit"), "should name the failing tool");
                assert!(msg.contains("5 consecutive times"), "should mention count");
            }
            other => panic!("Expected InjectWarning, got {:?}", other),
        }
    }

    #[test]
    fn test_failure_streak_reset_on_success() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        detector.record_call("edit", &make_args("a"), "error", false);
        detector.record_call("edit", &make_args("a"), "error", false);
        detector.record_call("edit", &make_args("b"), "success", true); // reset
        detector.record_call("edit", &make_args("a"), "error", false);
        detector.record_call("edit", &make_args("a"), "error", false);

        // Only 2 consecutive failures after reset → no trigger
        assert_eq!(detector.check(), LoopVerdict::Continue);
    }

    #[test]
    fn test_warning_then_hardstop() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        // First: inject warning (threshold is now 5)
        for _ in 0..5 {
            detector.record_call("grep", &make_args("x"), "nf", false);
        }
        assert!(matches!(detector.check(), LoopVerdict::InjectWarning(_)));

        // More of the same → HardStop
        for _ in 0..5 {
            detector.record_call("grep", &make_args("x"), "nf", false);
        }
        assert!(matches!(detector.check(), LoopVerdict::HardStop(_)));
    }

    #[test]
    fn test_disabled_strategies() {
        let mut detector = LoopDetector::new(LoopDetectionConfig {
            no_progress_threshold: 0,
            ping_pong_cycles: 0,
            failure_streak_threshold: 0,
            read_only_streak_threshold: 0,
            repeated_read_threshold: 0,
        });

        for _ in 0..10 {
            detector.record_call("grep", &make_args("x"), "nf", false);
        }
        assert_eq!(detector.check(), LoopVerdict::Continue);
    }

    #[test]
    fn test_utf8_safe_hash() {
        // CJK text should not panic
        let hash = hash_output("你好世界你好世界你好世界你好世界你好世界你好世界");
        // Just verify it returns a value without panicking
        assert!(hash > 0);
    }

    #[test]
    fn test_per_tool_failures_tracking() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        detector.record_call("tool_a", &make_args("1"), "error", false);
        detector.record_call("tool_a", &make_args("2"), "error", false);
        detector.record_call("tool_b", &make_args("1"), "error", false);

        assert_eq!(detector.failure_count("tool_a"), 2);
        assert_eq!(detector.failure_count("tool_b"), 1);

        detector.record_call("tool_a", &make_args("3"), "success", true);
        assert_eq!(detector.failure_count("tool_a"), 0, "reset on success");
        assert_eq!(detector.failure_count("tool_b"), 1, "other tool unaffected");
    }

    #[test]
    fn test_read_only_streak_different_files_no_trigger() {
        // 10 consecutive read-only calls, ALL different targets → valid exploration
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        let files = [
            "a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs", "g.rs", "h.rs", "i.rs", "j.rs",
        ];
        for f in &files {
            detector.record_call("read_file", &make_args(f), "content", true);
        }

        // 10 total, 10 unique (100%) → below 15 threshold, Continue
        assert_eq!(
            detector.check(),
            LoopVerdict::Continue,
            "reading 10 different files should not trigger"
        );
    }

    #[test]
    fn test_read_only_streak_repeated_same_target() {
        // Same (tool_name, args_sig) 3 times consecutively → Rule A triggers
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        // Some different reads first, then 3 identical
        detector.record_call("read_file", &make_args("a.rs"), "c1", true);
        detector.record_call("read_file", &make_args("b.rs"), "c2", true);
        detector.record_call("read_file", &make_args("a.rs"), "c1", true);
        detector.record_call("read_file", &make_args("a.rs"), "c1", true);
        detector.record_call("read_file", &make_args("a.rs"), "c1", true);

        match detector.check() {
            LoopVerdict::InjectWarning(msg) => {
                assert!(
                    msg.contains("Repeated read"),
                    "should mention repeated read, got: {}",
                    msg
                );
                assert!(msg.contains("3 consecutive"), "should mention count");
            }
            other => panic!("Expected InjectWarning for repeated read, got {:?}", other),
        }
    }

    #[test]
    fn test_read_only_streak_blind_exploration() {
        // 16 calls, only 3 unique targets (< 50%) → Rule B triggers
        let mut detector = LoopDetector::new(LoopDetectionConfig {
            read_only_streak_threshold: 15,
            repeated_read_threshold: 0, // disable Rule A for this test
            ..LoopDetectionConfig::default()
        });

        // Alternate 3 targets to keep unique < 50%
        for i in 0..16 {
            let target = match i % 3 {
                0 => "a.rs",
                1 => "b.rs",
                _ => "c.rs",
            };
            detector.record_call("read_file", &make_args(target), &format!("c{}", i), true);
        }

        // 16 total, 3 unique (18.75% < 50%) → Rule B
        match detector.check() {
            LoopVerdict::InjectWarning(msg) => {
                assert!(
                    msg.contains("Read-only streak"),
                    "should mention streak, got: {}",
                    msg
                );
                assert!(msg.contains("3 unique"), "should mention unique count");
            }
            other => panic!(
                "Expected InjectWarning for blind exploration, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_read_only_streak_many_different_files_above_threshold() {
        // 16 calls, 16 unique targets (100%) → no trigger even above total threshold
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        for i in 0..16 {
            detector.record_call(
                "read_file",
                &make_args(&format!("file_{}.rs", i)),
                "c",
                true,
            );
        }

        // 16 total >= 15, but 16 unique (100% >= 50%) → Continue
        assert_eq!(
            detector.check(),
            LoopVerdict::Continue,
            "16 different files should not trigger even above threshold"
        );
    }

    #[test]
    fn test_read_only_streak_broken_by_write() {
        let mut detector = LoopDetector::new(LoopDetectionConfig::default());

        detector.record_call("read_file", &make_args("a.rs"), "content", true);
        detector.record_call("read_file", &make_args("b.rs"), "content", true);
        detector.record_call("edit", &make_args("fix"), "success", true);
        detector.record_call("read_file", &make_args("c.rs"), "content", true);
        detector.record_call("read_file", &make_args("d.rs"), "content", true);

        assert_eq!(
            detector.check(),
            LoopVerdict::Continue,
            "write action breaks streak"
        );
    }

    #[test]
    fn test_read_only_streak_disabled() {
        let mut detector = LoopDetector::new(LoopDetectionConfig {
            no_progress_threshold: 0,
            ping_pong_cycles: 0,
            failure_streak_threshold: 0,
            read_only_streak_threshold: 0,
            repeated_read_threshold: 0,
        });

        for _ in 0..20 {
            detector.record_call("read_file", &make_args("same.rs"), "c", true);
        }
        assert_eq!(detector.check(), LoopVerdict::Continue);
    }
}
