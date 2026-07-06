#[cfg(test)]
mod tests {
    use crate::prompts::{
        get_default_system_prompt_with_date, get_research_system_prompt_with_date,
    };
    use time::Month;

    fn fixed_date() -> time::Date {
        time::Date::from_calendar_date(2026, Month::January, 15).unwrap()
    }

    #[test]
    fn test_get_default_system_prompt_no_context() {
        let date = fixed_date();
        let prompt = get_default_system_prompt_with_date(None, None, None, None, None, None, date);
        assert!(prompt.contains(&format!("SYSTEM: Today is {}", date)));
        assert!(prompt.contains("You are Shard, an AI assistant."));
        assert!(prompt.contains("Available Personas to Load (via `load_persona`):\nNone"));
        assert!(!prompt.contains("Some("));
    }

    #[test]
    fn test_get_default_system_prompt_with_memory() {
        let memory = "User prefers Rust for all coding tasks.";
        let prompt = get_default_system_prompt_with_date(
            Some(memory),
            None,
            None,
            None,
            None,
            None,
            fixed_date(),
        );
        assert!(prompt.contains(memory));
    }

    #[test]
    fn test_get_default_system_prompt_with_rag() {
        let rag = "The current project is a Tauri application.";
        let prompt = get_default_system_prompt_with_date(
            None,
            Some(rag),
            None,
            None,
            None,
            None,
            fixed_date(),
        );
        assert!(prompt.contains(rag));
    }

    #[test]
    fn test_get_default_system_prompt_with_both() {
        let memory = "User prefers Rust.";
        let rag = "Project is Shard.";
        let prompt = get_default_system_prompt_with_date(
            Some(memory),
            Some(rag),
            None,
            None,
            None,
            None,
            fixed_date(),
        );
        assert!(prompt.contains(memory));
        assert!(prompt.contains(rag));
    }

    #[test]
    fn test_get_research_system_prompt() {
        let date = fixed_date();
        let prompt = get_research_system_prompt_with_date(None, None, date);
        assert!(prompt.contains(&format!("SYSTEM: Today is {}", date)));
        assert!(prompt.contains("You are a Deep Research agent"));
    }

    #[test]
    fn test_prompt_with_peer_card() {
        let card = "## User Card\n\n- Lives in SF\n- Prefers Rust\n";
        let prompt = get_default_system_prompt_with_date(
            None,
            None,
            Some(card),
            None,
            None,
            None,
            fixed_date(),
        );
        assert!(prompt.contains("Lives in SF"));
        assert!(prompt.contains("Prefers Rust"));
    }

    #[test]
    fn test_prompt_with_peer_representation() {
        let rep = "## User Observations\n\n- Enjoys systems programming\n";
        let prompt = get_default_system_prompt_with_date(
            None,
            None,
            None,
            Some(rep),
            None,
            None,
            fixed_date(),
        );
        assert!(prompt.contains("Enjoys systems programming"));
    }

    #[test]
    fn test_prompt_with_all_context() {
        let memory = "User prefers Rust.";
        let rag = "Project is Shard.";
        let card = "## User Card\n- Lives in SF\n";
        let rep = "## Observations\n- Likes concise code\n";
        let prompt = get_default_system_prompt_with_date(
            Some(memory),
            Some(rag),
            Some(card),
            Some(rep),
            None,
            None,
            fixed_date(),
        );
        assert!(prompt.contains(memory));
        assert!(prompt.contains(rag));
        assert!(prompt.contains("Lives in SF"));
        assert!(prompt.contains("Likes concise code"));
    }
}
