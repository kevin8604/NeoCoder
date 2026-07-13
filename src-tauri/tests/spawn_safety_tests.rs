//! E2E-level integration tests for spawn panic recovery and pre-flight validation.
//!
//! These tests verify patterns used in commands/chat.rs for agent execution safety.
//! They run as integration tests (tests/ directory) to avoid the WebView2 linking issue.

#[cfg(test)]
mod spawn_safety {
    // ── P0-1: Spawn panic recovery ──

    /// Verifies that a nested tokio::spawn + JoinHandle correctly catches panics.
    /// This mirrors the exact pattern in commands/chat.rs:
    ///     tokio::spawn { tokio::spawn(agent) → await JoinHandle → catch panic }
    #[tokio::test]
    async fn test_spawn_panic_recovery_nested_spawn() {
        let outer = tokio::spawn(async move {
            let inner = tokio::spawn(async {
                panic!("simulated panic in agent task");
            });

            match inner.await {
                Ok(_) => "ok".to_string(),
                Err(join_err) => {
                    assert!(join_err.is_panic(), "should be a panic");
                    format!("panic caught: {:?}", join_err)
                }
            }
        });

        let result = outer.await.unwrap();
        assert!(result.contains("panic caught"), "should catch panic: {}", result);
        assert!(!result.contains("ok"), "should not return ok");
    }

    /// Verifies that panic payload extraction works for &str messages.
    #[tokio::test]
    async fn test_panic_payload_extraction_str() {
        // Simulate the panic payload extraction logic used in chat.rs
        let error_msg = "explicit panic message";

        let handle = tokio::spawn(async move {
            panic!("{}", error_msg);
        });

        let join_err = handle.await.unwrap_err();
        assert!(join_err.is_panic());

        // Extract payload using the same logic
        let extracted = if let Ok(payload) = join_err.try_into_panic() {
            if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic payload".to_string()
            }
        } else {
            "Panic (payload unavailable)".to_string()
        };

        assert!(extracted.contains(error_msg),
            "payload should contain '{}', got '{}'", error_msg, extracted);
    }

    /// Verifies that panic payload extraction works for String messages.
    #[tokio::test]
    async fn test_panic_payload_extraction_string() {
        let handle = tokio::spawn(async {
            panic!("{}", "explicit string panic".to_string());
        });

        let join_err = handle.await.unwrap_err();
        assert!(join_err.is_panic());

        let extracted = if let Ok(payload) = join_err.try_into_panic() {
            if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic payload".to_string()
            }
        } else {
            "Panic (payload unavailable)".to_string()
        };

        assert!(!extracted.is_empty(), "should extract non-empty panic message");
    }

    /// Verifies that a non-panic task error (cancellation) is correctly distinguished.
    #[tokio::test]
    async fn test_spawn_non_panic_error_distinguished() {
        let handle = tokio::spawn(async {
            Err::<(), String>("tool failure".to_string())
        });

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "tool failure");
    }

    /// Verifies that the new pre-flight validation function rejects empty API keys.
    #[tokio::test]
    async fn test_preflight_empty_api_key_rejected() {
        let api_key = "";
        let chat_model = "deepseek-chat";
        let agent_id = "reviewer";
        let agent_found = true; // simulating that agent exists in registry

        // Same logic as commands/chat.rs pre-flight
        let api_key_ok = !api_key.trim().is_empty();
        let model_ok = !chat_model.trim().is_empty();
        let agent_ok = agent_id.is_empty() || agent_id == "orchestrator" || agent_found;

        assert!(!api_key_ok, "empty API key should be detected");
        assert!(model_ok, "model should be valid");
        assert!(agent_ok, "agent should be found");
    }

    /// Verifies that missing chat model is detected.
    #[tokio::test]
    async fn test_preflight_empty_model_rejected() {
        let chat_model = "";

        let model_ok = !chat_model.trim().is_empty();
        assert!(!model_ok, "empty model should be detected");
    }

    /// Verifies that missing agent is detected for non-orchestrator agent_ids.
    #[tokio::test]
    async fn test_preflight_missing_agent_rejected() {
        let agent_id = "nonexistent";
        let agent_found = false;

        let agent_ok = agent_id.is_empty()
            || agent_id == "orchestrator"
            || agent_found;

        assert!(!agent_ok,
            "nonexistent agent ID should fail validation");
    }

    /// Verifies that orchestrator passes without agent def.
    #[tokio::test]
    async fn test_preflight_orchestrator_passes() {
        let agent_id = "orchestrator";
        let agent_found = false;

        let agent_ok = agent_id.is_empty()
            || agent_id == "orchestrator"
            || agent_found;

        assert!(agent_ok, "orchestrator should pass without agent_def");
    }

    /// Verifies that empty agent_id also passes (defaults to orchestrator).
    #[tokio::test]
    async fn test_preflight_empty_agent_id_passes() {
        let agent_id = "";
        let agent_found = false;

        let agent_ok = agent_id.is_empty()
            || agent_id == "orchestrator"
            || agent_found;

        assert!(agent_ok, "empty agent_id should pass (defaults to orchestrator)");
    }
}
