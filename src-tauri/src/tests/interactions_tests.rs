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

    #[test]
    fn test_cosine_similarity_logic() {
        // Test identical vectors
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);

        // Test orthogonal vectors
        let a = vec![1.0, 0.0, 0.0];
        let c = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &c) - 0.0).abs() < 1e-6);

        // Test opposite vectors
        let d = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &d) - (-1.0)).abs() < 1e-6);

        // Test zero vector (should return 0.0, not NaN)
        let zero = vec![0.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &zero), 0.0);

        // Test different length vectors (zip behavior)
        let a_short = vec![1.0, 2.0];
        let b_long = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a_short, &b_long);
        let expected = 5.0 / (5.0f32.sqrt() * 14.0f32.sqrt());
        assert!((sim - expected).abs() < 1e-6);
    }
}
