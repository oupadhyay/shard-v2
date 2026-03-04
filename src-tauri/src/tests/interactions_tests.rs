#[cfg(test)]
mod tests {
    use crate::interactions::*;
    use chrono::Utc;

    #[test]
    fn test_interaction_entry_serialization() {
        let entry = InteractionEntry {
            ts: Utc::now(),
            role: "user".to_string(),
            content: "Hello".to_string(),
            embedding: Some(vec![0.1, 0.2, 0.3]),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: InteractionEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(entry.role, deserialized.role);
        assert_eq!(entry.content, deserialized.content);
        assert_eq!(entry.embedding, deserialized.embedding);
    }
}
