//! Portable "knowledge pack": a project's accumulated tuning config and failure
//! lessons, exported as one JSON file a teammate can import to onboard fast.
//!
//! Deliberately scoped to the *learned state* (tuning knobs + lessons), not the
//! eval fixtures — those live in the repo and are shared via git already. This
//! keeps import purely additive to the local knowledge store, with no writes
//! into the project tree.
//!
//! The merge/serde logic here is pure and headless-testable; the file I/O and
//! Tauri commands wrap it.

use super::EvalFixture;
use crate::agent::failure_lessons::FailureLesson;
use crate::config::TuningConfig;
use serde::{Deserialize, Serialize};

/// Current pack schema version. Bump when the shape breaks compatibility.
pub const PACK_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgePack {
    pub version: u32,
    pub exported_at: String,
    pub tuning: TuningConfig,
    pub lessons: Vec<FailureLesson>,
    /// Count of eval fixtures that existed at export time (metadata only — the
    /// fixtures themselves are shared via the repo, not the pack).
    #[serde(default)]
    pub fixture_count: usize,
}

impl KnowledgePack {
    pub fn new(tuning: TuningConfig, lessons: Vec<FailureLesson>, fixture_count: usize) -> Self {
        Self {
            version: PACK_VERSION,
            exported_at: chrono::Utc::now().to_rfc3339(),
            tuning,
            lessons,
            fixture_count,
        }
    }
}

/// Merge an incoming pack's lessons into a local list. Union by
/// (project, signature): existing entries keep the higher `count`, the newer
/// `last_seen`, and fill in a location/symbol the local copy lacks; brand-new
/// signatures are appended. Returns how many lessons were newly added.
pub fn merge_lessons(target: &mut Vec<FailureLesson>, incoming: &[FailureLesson]) -> usize {
    let mut added = 0usize;
    for l in incoming {
        if let Some(existing) = target
            .iter_mut()
            .find(|e| e.project == l.project && e.signature == l.signature)
        {
            existing.count = existing.count.max(l.count);
            if l.last_seen > existing.last_seen {
                existing.last_seen = l.last_seen.clone();
            }
            if existing.file.is_none() && l.file.is_some() {
                existing.file = l.file.clone();
                existing.line = l.line;
                existing.symbol = l.symbol.clone();
            }
            if existing.snippet.is_empty() && !l.snippet.is_empty() {
                existing.snippet = l.snippet.clone();
            }
        } else {
            target.push(l.clone());
            added += 1;
        }
    }
    added
}

/// Parse a pack from JSON. Public so tests and commands share the shape check.
pub fn parse_pack(text: &str) -> Result<KnowledgePack, String> {
    let pack: KnowledgePack = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if pack.version > PACK_VERSION {
        return Err(format!(
            "pack version {} is newer than supported {}",
            pack.version, PACK_VERSION
        ));
    }
    Ok(pack)
}

/// Serialize a pack to pretty JSON for writing to disk.
pub fn to_json(pack: &KnowledgePack) -> Result<String, String> {
    serde_json::to_string_pretty(pack).map_err(|e| e.to_string())
}

/// Convenience: how many fixtures a dir contributes (kept out of the pack body,
/// recorded as metadata). Lives here so the command layer stays thin.
pub fn fixture_count_of(fixtures: &[EvalFixture]) -> usize {
    fixtures.len()
}

/// Result of importing a knowledge pack, for the UI to report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub lessons_added: usize,
    pub lessons_in_pack: usize,
    pub tuning_applied: bool,
    pub exported_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(project: &str, sig: &str, count: u32, seen: &str) -> FailureLesson {
        FailureLesson {
            project: project.into(),
            signature: sig.into(),
            tool: "run_build".into(),
            snippet: "x".into(),
            file: None,
            line: None,
            symbol: None,
            count,
            first_seen: seen.into(),
            last_seen: seen.into(),
        }
    }

    #[test]
    fn merge_adds_new_and_unions_existing() {
        let mut target = vec![lesson("/p", "E0308", 2, "2026-01-01T00:00:00Z")];
        let incoming = vec![
            lesson("/p", "E0308", 5, "2026-02-01T00:00:00Z"), // same sig → bump count+date
            lesson("/p", "E0433", 1, "2026-02-01T00:00:00Z"), // new sig
        ];
        let added = merge_lessons(&mut target, &incoming);
        assert_eq!(added, 1, "only E0433 is new");
        assert_eq!(target.len(), 2);
        let e0308 = target.iter().find(|l| l.signature == "E0308").unwrap();
        assert_eq!(e0308.count, 5, "keeps higher count");
        assert_eq!(
            e0308.last_seen, "2026-02-01T00:00:00Z",
            "keeps newer timestamp"
        );
    }

    #[test]
    fn merge_does_not_downgrade() {
        let mut target = vec![lesson("/p", "E0308", 9, "2026-05-01T00:00:00Z")];
        let incoming = vec![lesson("/p", "E0308", 1, "2026-01-01T00:00:00Z")];
        let added = merge_lessons(&mut target, &incoming);
        assert_eq!(added, 0);
        assert_eq!(target[0].count, 9, "local higher count wins");
        assert_eq!(
            target[0].last_seen, "2026-05-01T00:00:00Z",
            "newer local date kept"
        );
    }

    #[test]
    fn merge_fills_missing_location_from_incoming() {
        let mut target = vec![lesson("/p", "E0308", 1, "2026-01-01T00:00:00Z")];
        let mut incoming = lesson("/p", "E0308", 1, "2026-01-01T00:00:00Z");
        incoming.file = Some("src/a.rs".into());
        incoming.line = Some(7);
        incoming.symbol = Some("fn a".into());
        merge_lessons(&mut target, &[incoming]);
        assert_eq!(target[0].file.as_deref(), Some("src/a.rs"));
        assert_eq!(target[0].line, Some(7));
        assert_eq!(target[0].symbol.as_deref(), Some("fn a"));
    }

    #[test]
    fn pack_round_trips_through_json() {
        let pack = KnowledgePack::new(
            TuningConfig::default(),
            vec![lesson("/p", "E0308", 3, "t")],
            7,
        );
        let text = to_json(&pack).unwrap();
        let back = parse_pack(&text).unwrap();
        assert_eq!(back.version, PACK_VERSION);
        assert_eq!(back.lessons.len(), 1);
        assert_eq!(back.fixture_count, 7);
        assert_eq!(back.tuning.loop_no_progress, 5);
    }

    #[test]
    fn parse_rejects_too_new_version() {
        let mut pack = KnowledgePack::new(TuningConfig::default(), vec![], 0);
        pack.version = PACK_VERSION + 1;
        let text = to_json(&pack).unwrap();
        assert!(parse_pack(&text).is_err());
    }
}
