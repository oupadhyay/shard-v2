#[cfg(test)]
mod tests {
    use crate::skills::{get_skill_content, list_available_skills, get_skills_dir};
    use std::fs;

    #[test]
    fn test_list_available_skills_does_not_panic() {
        // Just ensuring it runs without panicking, even if directory is empty
        let _skills = list_available_skills();
    }

    #[test]
    fn test_get_skill_content_empty_name() {
        assert_eq!(get_skill_content(""), None);
    }

    #[test]
    fn test_get_skill_content_path_traversal() {
        assert_eq!(get_skill_content("../../../etc/passwd"), None);
        assert_eq!(get_skill_content("some/path"), None);
        assert_eq!(get_skill_content("some\\path"), None);
    }

    #[test]
    fn test_get_skill_content_nonexistent() {
        assert_eq!(get_skill_content("definitely_does_not_exist_skill_12345"), None);
    }

    #[test]
    fn test_skill_creation_and_retrieval() {
        if let Ok(dir) = get_skills_dir() {
            let test_skill_path = dir.join("test_skill_xyz.md");
            let test_content = "This is a test skill content.";

            // create a temporary skill file
            if fs::write(&test_skill_path, test_content).is_ok() {
                // Wait, list_available_skills doesn't currently take arbitrary paths, it uses get_skills_dir()
                let available = list_available_skills();
                assert!(available.contains(&"test_skill_xyz".to_string()));

                let content = get_skill_content("test_skill_xyz");
                assert_eq!(content.unwrap(), test_content);

                // cleanup
                let _ = fs::remove_file(&test_skill_path);
            }
        }
    }
}
