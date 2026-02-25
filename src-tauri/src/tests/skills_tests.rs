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

    #[test]
    fn test_skill_required_tools_extraction() {
        if let Ok(dir) = get_skills_dir() {
            // LF test
            let test_skill_path_lf = dir.join("test_tools_skill_lf.md");
            let test_content_lf = "---\nrequired_tools:\n  - tool_a\n  - tool_b\n---\nSkill content Here.";
            if fs::write(&test_skill_path_lf, test_content_lf).is_ok() {
                let tools = crate::skills::get_skill_required_tools("test_tools_skill_lf");
                assert_eq!(tools, vec!["tool_a".to_string(), "tool_b".to_string()]);
                let _ = fs::remove_file(&test_skill_path_lf);
            }

            // CRLF test
            let test_skill_path_crlf = dir.join("test_tools_skill_crlf.md");
            let test_content_crlf = "---\r\nrequired_tools:\r\n  - tool_c\r\n  - tool_d\r\n---\r\nSkill content Here.";
            if fs::write(&test_skill_path_crlf, test_content_crlf).is_ok() {
                let tools = crate::skills::get_skill_required_tools("test_tools_skill_crlf");
                assert_eq!(tools, vec!["tool_c".to_string(), "tool_d".to_string()]);
                let _ = fs::remove_file(&test_skill_path_crlf);
            }
        }
    }
}
