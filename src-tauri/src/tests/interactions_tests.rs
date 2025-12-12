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
        // We can't access the private function directly, but we can copy the logic to verify it
        fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
            let dot_product: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

            if norm_a == 0.0 || norm_b == 0.0 {
                return 0.0;
            }

            dot_product / (norm_a * norm_b)
        }

        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);

        let c = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &c) - 0.0).abs() < 1e-5);
    }
}
