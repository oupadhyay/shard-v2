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
    skills
}

/// Reads the content of a specific skill by its name.
pub fn get_skill_content(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }

    // Prevent path traversal
    if name.contains('/') || name.contains('\\') || name.contains("..") {
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
