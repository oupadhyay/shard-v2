#[cfg(test)]
mod tests {
    use crate::prompts::{
        get_default_system_prompt_with_date,
        get_research_system_prompt_with_date,
    };
    use time::Month;

    fn fixed_date() -> time::Date {
        time::Date::from_calendar_date(2026, Month::January, 15).unwrap()
    }

    #[test]
    fn test_get_default_system_prompt_no_context() {
        let date = fixed_date();
        let prompt = get_default_system_prompt_with_date(None, None, None, date);
        assert!(prompt.contains(&format!("SYSTEM: Today is {}", date)));
        assert!(prompt.contains("You are Shard, an AI assistant."));
        assert!(!prompt.contains("None"));
        assert!(!prompt.contains("Some("));
    }

    #[test]
    fn test_get_default_system_prompt_with_memory() {
        let memory = "User prefers Rust for all coding tasks.";
        let prompt = get_default_system_prompt_with_date(Some(memory), None, None, fixed_date());
        assert!(prompt.contains(memory));
    }

    #[test]
    fn test_get_default_system_prompt_with_rag() {
        let rag = "The current project is a Tauri application.";
        let prompt = get_default_system_prompt_with_date(None, Some(rag), None, fixed_date());
        assert!(prompt.contains(rag));
    }

    #[test]
    fn test_get_default_system_prompt_with_both() {
        let memory = "User prefers Rust.";
        let rag = "Project is Shard.";
        let prompt = get_default_system_prompt_with_date(Some(memory), Some(rag), None, fixed_date());
        assert!(prompt.contains(memory));
        assert!(prompt.contains(rag));
    }

    #[test]
    fn test_get_research_system_prompt() {
        let date = fixed_date();
        let prompt = get_research_system_prompt_with_date(None, date);
        assert!(prompt.contains(&format!("SYSTEM: Today is {}", date)));
        assert!(prompt.contains("You are a Deep Research agent"));
    }
}
