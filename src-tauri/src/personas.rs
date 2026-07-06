use std::fs;
use std::path::PathBuf;

/// Tier-1 metadata for persona listing (cheap to compute).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonaMetadata {
    pub name: String,
    pub description: Option<String>,
    pub required_tools: Vec<String>,
    pub category: Option<String>,
}

/// Gets the directory path where personas are stored.
/// If it doesn't exist, it attempts to create it.
pub fn get_personas_dir() -> Result<PathBuf, String> {
    if let Some(mut base_dir) = dirs::data_local_dir() {
        base_dir.push("dev.ojasw.shard");

        let personas_dir = base_dir.join("personas");

        if !personas_dir.exists() {
            if let Err(e) = fs::create_dir_all(&personas_dir) {
                return Err(format!("Failed to create personas directory: {}", e));
            }
        }

        Ok(personas_dir)
    } else {
        Err("Could not find local data directory for personas".to_string())
    }
}

/// Returns a list of base filenames (personas) found in the personas directory.
pub fn list_available_personas() -> Vec<String> {
    let mut personas = Vec::new();
    if let Ok(dir) = get_personas_dir() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        personas.push(stem.to_string());
                    }
                }
            }
        }
    }
    personas.sort();
    personas
}

/// Validates a filename to ensure it's a single "normal" path component.
/// This prevents path traversal by rejecting names with separators, ".." components, etc.
pub(crate) fn is_safe_filename(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    // Explicitly block separators, drive prefixes, and control characters
    // that might allow escaping the intended directory.
    if name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.chars().any(|c| c.is_control())
    {
        return false;
    }

    let path = std::path::Path::new(name);
    let mut components = path.components();

    match components.next() {
        Some(std::path::Component::Normal(c)) => {
            // Must be a single normal component matching the input exactly.
            c.to_str() == Some(name) && components.next().is_none()
        }
        _ => false,
    }
}

/// Reads the content of a specific persona by its name.
pub fn get_persona_content(name: &str) -> Option<String> {
    if !is_safe_filename(name) {
        return None;
    }

    if let Ok(mut path) = get_personas_dir() {
        path.push(format!("{}.md", name));
        if path.exists() && path.is_file() {
            return fs::read_to_string(path).ok();
        }
    }
    None
}

/// Parses a persona's Markdown file and extracts the "required_tools" list from its YAML frontmatter.
/// Example frontmatter:
/// ---
/// required_tools:
///   - search_arxiv
///   - get_weather
/// ---
pub fn get_persona_required_tools(name: &str) -> Vec<String> {
    let mut required = Vec::new();

    if let Some(content) = get_persona_content(name) {
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
                            let tool = trimmed.strip_prefix('-').unwrap().trim();
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

/// List all available personas, supporting both flat files and directory-based skills.
/// Flat: `personas/foo.md` → "foo"
/// Directory: `personas/bar/SKILL.md` → "bar"
/// Nested: `personas/category/bar/SKILL.md` → "category/bar"
pub fn list_available_personas_v2() -> Vec<String> {
    let mut personas = Vec::new();
    if let Ok(dir) = get_personas_dir() {
        collect_personas_recursive(&dir, &dir, &mut personas);
    }
    personas.sort();
    personas
}

/// Recursively scan for flat `.md` files and `SKILL.md` directory-based personas.
fn collect_personas_recursive(
    base: &std::path::Path,
    current: &std::path::Path,
    out: &mut Vec<String>,
) {
    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                // Skip SKILL.md files — they're handled via their parent directory
                if path.file_name().and_then(|f| f.to_str()) == Some("SKILL.md") {
                    // Register the parent directory as a persona
                    if let Ok(relative) = current.strip_prefix(base) {
                        let name = relative.to_string_lossy().to_string();
                        if !name.is_empty() && name.split('/').all(is_safe_filename) {
                            out.push(name);
                        }
                    }
                } else if current == base {
                    // Flat .md file at the top level
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        out.push(stem.to_string());
                    }
                }
            }
        } else if path.is_dir() {
            // Validate directory name component
            if let Some(dir_name) = path.file_name().and_then(|f| f.to_str()) {
                if is_safe_filename(dir_name) {
                    collect_personas_recursive(base, &path, out);
                }
            }
        }
    }
}

/// Resolve a persona name to its content, checking both flat and directory layouts.
/// Tries: `{name}.md` first, then `{name}/SKILL.md`.
pub fn resolve_persona_content(name: &str) -> Option<String> {
    // Validate each path component
    for component in name.split('/') {
        if !is_safe_filename(component) {
            return None;
        }
    }

    let dir = get_personas_dir().ok()?;

    // Try flat file first (only for single-component names)
    if !name.contains('/') {
        let flat_path = dir.join(format!("{}.md", name));
        if flat_path.is_file() {
            return fs::read_to_string(flat_path).ok();
        }
    }

    // Try directory-based SKILL.md
    let skill_path = dir.join(name).join("SKILL.md");
    if skill_path.is_file() {
        return fs::read_to_string(skill_path).ok();
    }

    None
}

/// Parse YAML frontmatter fields from persona content.
/// Returns (description, required_tools, category).
fn parse_frontmatter_fields(content: &str) -> (Option<String>, Vec<String>, Option<String>) {
    let mut description = None;
    let mut required_tools = Vec::new();
    let mut category = None;

    if content.starts_with("---\n") || content.starts_with("---\r\n") {
        let prefix_len = if content.starts_with("---\r\n") { 5 } else { 4 };
        let end_index = content[prefix_len..].find("\n---").map(|i| i + prefix_len);
        if let Some(end) = end_index {
            let frontmatter = &content[prefix_len..end];
            let mut in_required_tools = false;

            for line in frontmatter.lines() {
                let trimmed = line.trim();

                if trimmed.starts_with("description:") {
                    in_required_tools = false;
                    let val = trimmed.strip_prefix("description:").unwrap().trim();
                    let val = val.trim_matches(|c| c == '\'' || c == '"');
                    if !val.is_empty() {
                        description = Some(val.to_string());
                    }
                } else if trimmed.starts_with("category:") {
                    in_required_tools = false;
                    let val = trimmed.strip_prefix("category:").unwrap().trim();
                    let val = val.trim_matches(|c| c == '\'' || c == '"');
                    if !val.is_empty() {
                        category = Some(val.to_string());
                    }
                } else if trimmed.starts_with("required_tools:") {
                    in_required_tools = true;
                } else if in_required_tools {
                    if trimmed.starts_with('-') {
                        let tool = trimmed.strip_prefix('-').unwrap().trim();
                        let tool = tool.trim_matches(|c| c == '\'' || c == '"');
                        if !tool.is_empty() {
                            required_tools.push(tool.to_string());
                        }
                    } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        in_required_tools = false;
                    }
                }
            }
        }
    }

    (description, required_tools, category)
}

/// Extract metadata from a persona's YAML frontmatter without loading full content.
/// Parses: `description`, `required_tools`, `category` from the YAML block.
pub fn get_persona_metadata(name: &str) -> Option<PersonaMetadata> {
    let content = resolve_persona_content(name).or_else(|| get_persona_content(name))?;

    let (description, required_tools, category) = parse_frontmatter_fields(&content);

    Some(PersonaMetadata {
        name: name.to_string(),
        description,
        required_tools,
        category,
    })
}

/// List all personas with their Tier-1 metadata.
pub fn list_personas_with_metadata() -> Vec<PersonaMetadata> {
    list_available_personas_v2()
        .into_iter()
        .filter_map(|name| get_persona_metadata(&name))
        .collect()
}

/// Security: scan persona content for prompt injection patterns.
/// Returns a list of warnings (empty = clean).
pub fn scan_persona_content(content: &str) -> Vec<String> {
    let mut warnings = Vec::new();

    // Check for invisible Unicode characters (zero-width spaces, etc.)
    for (i, c) in content.char_indices() {
        if matches!(
            c,
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{2060}' | '\u{00AD}'
        ) {
            warnings.push(format!(
                "Invisible Unicode character U+{:04X} at byte offset {}",
                c as u32, i
            ));
        }
    }

    // Check for common prompt injection patterns
    let injection_patterns = [
        "ignore previous instructions",
        "ignore all previous",
        "disregard your instructions",
        "you are now",
        "new instructions:",
        "system prompt:",
        "IMPORTANT: ",
    ];

    let lower = content.to_lowercase();
    for pattern in &injection_patterns {
        if lower.contains(&pattern.to_lowercase()) {
            warnings.push(format!("Potential prompt injection pattern: '{}'", pattern));
        }
    }

    warnings
}
