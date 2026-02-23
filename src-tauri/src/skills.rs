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

/// Helper function to perform a recursive walk for .md files
#[allow(dead_code)]
fn walk_dir_for_skills(dir: &PathBuf, max_depth: usize, current_depth: usize) -> Vec<PathBuf> {
    let mut matching_files = Vec::new();
    if current_depth > max_depth {
        return matching_files;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                matching_files.extend(walk_dir_for_skills(&path, max_depth, current_depth + 1));
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                matching_files.push(path);
            }
        }
    }
    matching_files
}

/// Loads all active skills (.md files) from the skills directory.
/// Does not watch them live yet—loads them on demand.
pub fn load_active_skills() -> String {
    // Disabled as per user request until model-selectable
    String::new()
}
