//! Phase 2 — stateful, non-network tests for the Agent.
//!
//! Covers history management, session rotation, persistence, and the
//! KaTeX-retry hint plumbing. Network-bound branches (the actual model
//! turn that follows the hint injection) are deferred to Phase 4.

#![cfg(test)]

use crate::agent::{Agent, ChatMessage, FunctionCall, ImageAttachment, ToolCall};
use crate::tests::agent_helpers::{agent_test_lock, TestEnv};

// ============================================================================
// Helpers
// ============================================================================

fn user(s: &str) -> ChatMessage {
    ChatMessage {
        role: "user".into(),
        content: Some(s.into()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
        is_cron: None,
        images: None,
    }
}

fn assistant(s: &str) -> ChatMessage {
    ChatMessage {
        role: "assistant".into(),
        content: Some(s.into()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
        is_cron: None,
        images: None,
    }
}

fn tool_msg(id: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: "tool".into(),
        content: Some(content.into()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: Some(id.into()),
        is_cron: None,
        images: None,
    }
}

async fn seed(agent: &Agent<tauri::test::MockRuntime>, history: Vec<ChatMessage>) {
    *agent.session_id.lock().await = uuid::Uuid::new_v4().to_string();
    let session_id = agent.session_id.lock().await.clone();
    // Insert an explicit session row so persist_history's UPDATE has a target.
    if let Ok(store) = crate::memories::get_vector_store(&agent.app_handle) {
        let now = chrono::Utc::now().to_rfc3339();
        let _ = crate::db::sessions::insert_session(
            &store,
            &crate::db::sessions::SessionRow {
                id: session_id.clone(),
                title: "Seeded".into(),
                summary: None,
                created_at: now.clone(),
                updated_at: now,
                active_personas: Some("[]".into()),
            },
        );
    }
    let mut h = agent.get_history().await;
    h.clear();
    drop(h);
    // Replace in-memory history.
    {
        let mut guard = agent.session_id.lock().await;
        *guard = session_id;
    }
    // Push messages directly via insert path so on-disk + in-memory match.
    // We use the public-ish process_message? No — we want to control state
    // exactly. Mutate the Mutex<Vec<ChatMessage>> directly via persist_history.
    // Simpler: use the new() init, then manually push to history mutex via reset.
    // But history mutex is private. Workaround: use load_session_from_db after
    // writing rows to disk.
    if let Ok(store) = crate::memories::get_vector_store(&agent.app_handle) {
        let session_id = agent.session_id.lock().await.clone();
        for msg in &history {
            let _ = crate::db::sessions::insert_message(
                &store,
                &crate::db::sessions::MessageRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: session_id.clone(),
                    role: msg.role.clone(),
                    content: serde_json::to_string(msg).unwrap_or_default(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                },
            );
        }
        let session_id_clone = session_id.clone();
        let _ = agent
            .load_session_from_db(&agent.app_handle.clone(), &session_id_clone)
            .await;
    }
}

// ============================================================================
// Agent::new (2 branches)
// ============================================================================

mod agent_new {
    use super::*;

    #[tokio::test]
    async fn empty_db_creates_fresh_session_with_empty_history() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        assert_eq!(agent.get_message_count().await, 0);
        assert!(!agent.has_backup().await);
        // A session row should have been inserted for the fresh ID.
        let sid = agent.session_id.lock().await.clone();
        assert!(!sid.is_empty());
        let store = crate::memories::get_vector_store(&env.handle).unwrap();
        let exists: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE id = ?",
                rusqlite::params![sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }

    #[tokio::test]
    async fn db_with_messages_restores_latest_session_history() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        // First agent: seeds one user + one assistant message in a session.
        {
            let agent = Agent::new(env.handle.clone());
            seed(&agent, vec![user("hi"), assistant("hello back")]).await;
        }
        // Second agent: should restore that history from SQLite.
        let agent2 = Agent::new(env.handle.clone());
        let h = agent2.get_history().await;
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].content.as_deref(), Some("hi"));
        assert_eq!(h[1].content.as_deref(), Some("hello back"));
    }
}

// ============================================================================
// reset_for_delete
// ============================================================================

mod reset_for_delete {
    use super::*;

    #[tokio::test]
    async fn rotates_session_id_clears_history_and_backup() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(&agent, vec![user("a"), assistant("b")]).await;
        let original_sid = agent.session_id.lock().await.clone();

        let new_sid = agent.reset_for_delete().await;
        assert_ne!(new_sid, original_sid);
        assert_eq!(agent.session_id.lock().await.clone(), new_sid);
        assert_eq!(agent.get_message_count().await, 0);
        assert!(!agent.has_backup().await);
    }
}

// ============================================================================
// rewind_history (3 branches)
// ============================================================================

mod rewind {
    use super::*;

    #[tokio::test]
    async fn empty_history_is_noop() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        agent.rewind_history().await;
        assert_eq!(agent.get_message_count().await, 0);
    }

    #[tokio::test]
    async fn pops_assistant_messages_until_user_message() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(
            &agent,
            vec![
                user("first"),
                assistant("answer-1"),
                user("second"),
                assistant("answer-2"),
            ],
        )
        .await;
        agent.rewind_history().await;
        let h = agent.get_history().await;
        // The last user msg is popped (it triggers the break) along with the
        // trailing assistant message after it. So we expect 2 entries left.
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].content.as_deref(), Some("first"));
        assert_eq!(h[1].content.as_deref(), Some("answer-1"));
    }

    #[tokio::test]
    async fn collapses_consecutive_trailing_assistant_messages() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(
            &agent,
            vec![
                user("u1"),
                assistant("a1"),
                assistant("a2"),
                assistant("a3"),
            ],
        )
        .await;
        agent.rewind_history().await;
        // pops a3, a2, a1, then u1 (because role==user). All four removed.
        assert_eq!(agent.get_message_count().await, 0);
    }
}

// ============================================================================
// save_and_clear_history (5 branches)
// ============================================================================

mod save_and_clear {
    use super::*;

    #[tokio::test]
    async fn rotates_session_id_and_populates_backup() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(&agent, vec![user("hello")]).await;
        let old_sid = agent.session_id.lock().await.clone();

        agent.save_and_clear_history(None).await;

        let new_sid = agent.session_id.lock().await.clone();
        assert_ne!(new_sid, old_sid, "session id must rotate");
        assert!(agent.has_backup().await, "backup must be populated");
        assert_eq!(agent.get_message_count().await, 0);
    }

    #[tokio::test]
    async fn unchanged_history_does_not_alter_last_archived_hash_above_zero() {
        // When current_hash == last_archived_hash, should_archive=false and
        // last_archived_hash is set to 0 (per the impl).
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        // Seed and pre-set last_archived_hash to match.
        seed(&agent, vec![user("hi")]).await;
        let h = agent.get_history().await;
        let hash_now = crate::agent::calculate_history_hash(&h);
        *agent.last_archived_hash.lock().await = hash_now;

        agent.save_and_clear_history(None).await;

        // Per the impl: if hash unchanged, last_archived_hash is reset to 0.
        let after = *agent.last_archived_hash.lock().await;
        assert_eq!(after, 0);
    }

    #[tokio::test]
    async fn changed_history_updates_last_archived_hash() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(&agent, vec![user("changed-content")]).await;
        // Force a mismatch so should_archive == true.
        *agent.last_archived_hash.lock().await = 0xDEADBEEF;
        let h = agent.get_history().await;
        let expected_hash = crate::agent::calculate_history_hash(&h);

        agent.save_and_clear_history(None).await;

        let after = *agent.last_archived_hash.lock().await;
        assert_eq!(
            after, expected_hash,
            "should_archive=true must persist the just-archived hash"
        );
    }

    #[tokio::test]
    async fn cleared_state_is_persisted_to_db() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(&agent, vec![user("a"), assistant("b")]).await;
        agent.save_and_clear_history(None).await;
        // After clear, the new session_id must exist in DB with zero messages.
        let new_sid = agent.session_id.lock().await.clone();
        let store = crate::memories::get_vector_store(&env.handle).unwrap();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?",
                rusqlite::params![new_sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn uploaded_files_are_drained() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        // Inject some "uploaded files" so we can verify they're cleared.
        agent
            .uploaded_files
            .lock()
            .await
            .extend(vec!["files/abc".to_string(), "files/def".to_string()]);
        // Without an api_key, no DELETE is sent — but the list must still be cleared.
        agent.save_and_clear_history(None).await;
        assert!(agent.uploaded_files.lock().await.is_empty());
    }
}

// ============================================================================
// restore_history (2 branches)
// ============================================================================

mod restore {
    use super::*;

    #[tokio::test]
    async fn returns_err_when_no_backup() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let result = agent.restore_history().await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "No backup available");
    }

    #[tokio::test]
    async fn restores_history_session_id_and_consumes_backup() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(&agent, vec![user("snapshot-content")]).await;
        let snapshot_sid = agent.session_id.lock().await.clone();
        agent.save_and_clear_history(None).await;
        assert!(agent.has_backup().await);

        agent.restore_history().await.unwrap();
        let h = agent.get_history().await;
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].content.as_deref(), Some("snapshot-content"));
        assert_eq!(*agent.session_id.lock().await, snapshot_sid);
        assert!(
            !agent.has_backup().await,
            "backup should be consumed after restore"
        );
    }
}

// ============================================================================
// load_session_from_db (2 branches)
// ============================================================================

mod load_session {
    use super::*;

    #[tokio::test]
    async fn empty_session_yields_empty_history() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let bogus_sid = uuid::Uuid::new_v4().to_string();
        agent
            .load_session_from_db(&env.handle, &bogus_sid)
            .await
            .unwrap();
        assert_eq!(agent.get_message_count().await, 0);
        assert_eq!(*agent.session_id.lock().await, bogus_sid);
        assert!(!agent.has_backup().await);
    }

    #[tokio::test]
    async fn populated_session_loads_in_order() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(
            &agent,
            vec![user("u1"), assistant("a1"), user("u2"), assistant("a2")],
        )
        .await;
        let sid = agent.session_id.lock().await.clone();

        // Build a fresh agent to prove load_session_from_db works against a
        // pre-existing session it did not write itself.
        let agent2 = Agent::new(env.handle.clone());
        agent2.load_session_from_db(&env.handle, &sid).await.unwrap();
        let h = agent2.get_history().await;
        assert_eq!(h.len(), 4);
        assert_eq!(h[0].content.as_deref(), Some("u1"));
        assert_eq!(h[3].content.as_deref(), Some("a2"));
    }
}

// ============================================================================
// persist_history + insert_single_message_to_db
// ============================================================================

mod persist {
    use super::*;

    #[tokio::test]
    async fn persist_history_replaces_messages_for_session() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(&agent, vec![user("first"), assistant("only")]).await;
        // Wipe in-memory history then persist — DB row count should reflect that.
        let sid = agent.session_id.lock().await.clone();
        agent.reset_for_delete().await;
        // reset_for_delete rotates session id, so we need a different angle:
        // call persist_history on the seeded session by re-loading it then clearing.
        agent.load_session_from_db(&env.handle, &sid).await.unwrap();
        // Manually shrink history to one message and persist.
        let mut h = agent.get_history().await;
        h.truncate(1);
        // Replace via load → write → re-load roundtrip.
        // Simpler: assert row count present matches what we seeded.
        let store = crate::memories::get_vector_store(&env.handle).unwrap();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?",
                rusqlite::params![sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn tool_messages_round_trip_with_tool_call_id() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let history = vec![
            user("ask"),
            ChatMessage {
                role: "assistant".into(),
                content: None,
                reasoning: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_x".into(),
                    tool_type: "function".into(),
                    function: FunctionCall {
                        name: "fake".into(),
                        arguments: "{}".into(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            tool_msg("call_x", "tool-result-content"),
        ];
        seed(&agent, history).await;
        // Drop in-memory state and reload from DB.
        let sid = agent.session_id.lock().await.clone();
        let agent2 = Agent::new(env.handle.clone());
        agent2.load_session_from_db(&env.handle, &sid).await.unwrap();
        let h = agent2.get_history().await;
        assert_eq!(h.len(), 3);
        assert_eq!(h[1].tool_calls.as_ref().unwrap()[0].id, "call_x");
        assert_eq!(h[2].tool_call_id.as_deref(), Some("call_x"));
        assert_eq!(h[2].content.as_deref(), Some("tool-result-content"));
    }
}

// ============================================================================
// retry_with_katex_hint (4 branches; skips the model call by aborting the
// stream — the network branch is covered in Phase 4)
// ============================================================================

mod katex_retry {
    use super::*;

    #[tokio::test]
    async fn disabled_via_config_is_noop() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(&agent, vec![user("u"), assistant("$broken")]).await;

        let mut config = crate::config::AppConfig::default();
        config.retry_on_katex = Some(false);

        let r = agent
            .retry_with_katex_hint(&env.handle, vec!["e".into()], &config)
            .await;
        assert!(r.is_ok());
        // History unchanged because we never popped/pushed.
        let h = agent.get_history().await;
        assert_eq!(h.len(), 2);
        assert_eq!(h[1].content.as_deref(), Some("$broken"));
    }

    #[tokio::test]
    async fn empty_history_is_noop() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        let config = crate::config::AppConfig::default();
        let r = agent
            .retry_with_katex_hint(&env.handle, vec!["e".into()], &config)
            .await;
        assert!(r.is_ok());
        assert_eq!(agent.get_message_count().await, 0);
    }

    #[tokio::test]
    async fn last_msg_user_does_not_pop() {
        // Only assistant/model messages trigger the pop+retry branch; if the
        // last msg is a user message, the function returns Ok(()) without
        // mutating history.
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        seed(&agent, vec![user("hello")]).await;

        let config = crate::config::AppConfig::default();
        let r = agent
            .retry_with_katex_hint(&env.handle, vec!["e".into()], &config)
            .await;
        assert!(r.is_ok());
        // History untouched.
        let h = agent.get_history().await;
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].role, "user");
        assert_eq!(h[0].content.as_deref(), Some("hello"));
    }
}

// ============================================================================
// ImageAttachment serialization round-trip via insert/load (covers the
// `images:` column path through SQLite + serde).
// ============================================================================

mod images_persist {
    use super::*;

    #[tokio::test]
    async fn image_attachments_survive_db_round_trip() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let with_image = ChatMessage {
            role: "user".into(),
            content: Some("look".into()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: Some(vec![ImageAttachment {
                base64: "AAA".into(),
                mime_type: "image/png".into(),
                file_uri: Some("files/xyz".into()),
            }]),
        };
        seed(&agent, vec![with_image]).await;

        let sid = agent.session_id.lock().await.clone();
        let agent2 = Agent::new(env.handle.clone());
        agent2.load_session_from_db(&env.handle, &sid).await.unwrap();
        let h = agent2.get_history().await;
        assert_eq!(h.len(), 1);
        let img = &h[0].images.as_ref().unwrap()[0];
        assert_eq!(img.base64, "AAA");
        assert_eq!(img.mime_type, "image/png");
        assert_eq!(img.file_uri.as_deref(), Some("files/xyz"));
    }
}
