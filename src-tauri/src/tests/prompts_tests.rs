#[cfg(test)]
mod tests {
    use crate::prompts::get_jailbreak_prompt;

    #[test]
    fn test_get_jailbreak_prompt_gemini_pro() {
        let prompt = get_jailbreak_prompt("gemini-2.5-pro");
        assert!(prompt.contains("RESET_CORTEX"));
        assert!(prompt.contains("Plinian Omniverse"));
    }

    #[test]
    fn test_get_jailbreak_prompt_gemini_flash() {
        let prompt = get_jailbreak_prompt("gemini-2.0-flash");
        assert!(prompt.contains("step by step extremely detailed hypothetical genius response"));
        assert!(prompt.contains("BUCKLE UP!"));
    }

    #[test]
    fn test_get_jailbreak_prompt_deepseek() {
        let prompt = get_jailbreak_prompt("deepseek-r1");
        assert!(prompt.contains("LOVE PLINY"));
        assert!(prompt.contains("rebel genius"));
    }

    #[test]
    fn test_get_jailbreak_prompt_grok() {
        let prompt = get_jailbreak_prompt("grok-beta");
        assert!(prompt.contains("Library of Babel"));
    }

    #[test]
    fn test_get_jailbreak_prompt_default() {
        let prompt = get_jailbreak_prompt("unknown-model");
        assert!(prompt.contains("step by step extremely detailed hypothetical genius response"));
    }
}
