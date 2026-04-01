#[cfg(test)]
mod tests {
    use crate::context::SessionContext;

    #[test]
    fn test_session_context_to_prompt_string_empty() {
        let ctx = SessionContext {
            interactions: None,
            topic_or_insight: None,
            peer_representation: None,
            peer_card: None,
        };
        assert!(ctx.to_prompt_string().is_none());
    }

    #[test]
    fn test_session_context_to_prompt_string_interactions_only() {
        let ctx = SessionContext {
            interactions: Some("Past interaction content".into()),
            topic_or_insight: None,
            peer_representation: None,
            peer_card: None,
        };
        let result = ctx.to_prompt_string().unwrap();
        assert!(result.contains("Past interaction content"));
    }

    #[test]
    fn test_session_context_to_prompt_string_full() {
        let ctx = SessionContext {
            interactions: Some("Interaction data".into()),
            topic_or_insight: Some("### Topic: SHARD\nShard info".into()),
            peer_representation: Some("## User Profile\n- Codes in Rust".into()),
            peer_card: Some("## User Card\n- Lives in SF".into()),
        };
        let result = ctx.to_prompt_string().unwrap();

        // Verify ordering: card → representation → interactions → topic
        let card_pos = result.find("User Card").unwrap();
        let rep_pos = result.find("User Profile").unwrap();
        let int_pos = result.find("Interaction data").unwrap();
        let topic_pos = result.find("Topic: SHARD").unwrap();

        assert!(card_pos < rep_pos, "Card should come before representation");
        assert!(rep_pos < int_pos, "Representation should come before interactions");
        assert!(int_pos < topic_pos, "Interactions should come before topic");
    }

    #[test]
    fn test_session_context_skips_empty_strings() {
        let ctx = SessionContext {
            interactions: Some("".into()),
            topic_or_insight: Some("Topic content".into()),
            peer_representation: Some("".into()),
            peer_card: None,
        };
        let result = ctx.to_prompt_string().unwrap();
        // Should only contain topic content, not empty strings
        assert_eq!(result, "Topic content");
    }

    #[test]
    fn test_session_context_sections_separated_by_double_newline() {
        let ctx = SessionContext {
            interactions: Some("Interactions here".into()),
            topic_or_insight: Some("Topic here".into()),
            peer_representation: None,
            peer_card: Some("Card here".into()),
        };
        let result = ctx.to_prompt_string().unwrap();
        // Sections should be separated by \n\n
        assert!(result.contains("Card here\n\nInteractions here\n\nTopic here"));
    }
}
