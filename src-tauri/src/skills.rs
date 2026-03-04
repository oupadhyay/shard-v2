use std::fs;
use std::path::PathBuf;

/// Gets the directory path where skills are stored.
/// If it doesn't exist, it attempts to create it.
pub fn get_skills_dir() -> Result<PathBuf, String> {
    if let Some(mut base_dir) = dirs::data_local_dir() {
        base_dir.push("dev.ojasw.shard");
        base_dir.push("skills");

        if !base_dir.exists() {
            if let Err(e) = fs::create_dir_all(&base_dir) {
                return Err(format!("Failed to create skills directory: {}", e));
            }
        }
        Ok(base_dir)
    } else {
        Err("Could not find local data directory for skills".to_string())
    }
}

/// Returns a list of base filenames (skills) found in the skills directory.
pub fn list_available_skills() -> Vec<String> {
    let mut skills = Vec::new();
    if let Ok(dir) = get_skills_dir() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        skills.push(stem.to_string());
                    }
                }
            }
        }
    }
    skills.sort();
    skills
}

/// Validates a filename to ensure it's a single "normal" path component.
/// This prevents path traversal by rejecting names with separators, ".." components, etc.
pub fn is_safe_filename(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    // Explicitly block separators and drive prefixes that might be parsed as
    // part of a single Normal component on some platforms but still allow
    // escaping the intended directory via PathBuf::push.
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return false;
    }

    let path = std::path::Path::new(name);
    let mut components = path.components();

    match components.next() {
        Some(std::path::Component::Normal(c)) => {
            // Must be a normal component, and it must exactly match the input
            // to ensure no other components (like separators) are present.
            c == name && components.next().is_none()
        }
        _ => false,
    }
}

/// Reads the content of a specific skill by its name.
pub fn get_skill_content(name: &str) -> Option<String> {
    if !is_safe_filename(name) {
        return None;
    }

    if let Ok(mut path) = get_skills_dir() {
        path.push(format!("{}.md", name));
        if path.exists() && path.is_file() {
            return fs::read_to_string(path).ok();
        }
    }
    None
}

/// Parses a skill's Markdown file and extracts the "required_tools" list from its YAML frontmatter.
/// Example frontmatter:
/// ---
/// required_tools:
///   - search_arxiv
///   - get_weather
/// ---
pub fn get_skill_required_tools(name: &str) -> Vec<String> {
    let mut required = Vec::new();

    if let Some(content) = get_skill_content(name) {
        if content.starts_with("---\n") || content.starts_with("---\r\n") {
            let prefix_len = if content.starts_with("---\r\n") { 5 } else { 4 };
            // Find the end of the frontmatter
            let end_index = content[prefix_len..].find("\n---").map(|i| i + prefix_len);
            if let Some(end) = end_index {
                let frontmatter = &content[prefix_len..end];
                let mut in_required_tools = false;

                for line in frontmatter.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("required_tools:") {
                        in_required_tools = true;
                    } else if in_required_tools {
                        if trimmed.starts_with('-') {
                            let tool = trimmed[1..].trim();
                            // If it's wrapped in quotes, strip them
                            let tool = tool.trim_matches(|c| c == '\'' || c == '"');
                            if !tool.is_empty() {
                                required.push(tool.to_string());
                            }
                        } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                            // Reached the next key in the YAML or something that breaks the list
                            break;
                        }
                    }
                }
            }
        }
    }

    required
}
