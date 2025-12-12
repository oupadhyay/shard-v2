
use crate::prompts::get_research_system_prompt;

#[test]
fn test_research_prompt_integrity() {
    let prompt = get_research_system_prompt();
    assert!(prompt.contains("Deep Research agent"));
    assert!(prompt.contains("Produce an initial research plan"));
    assert!(prompt.contains("Execute iteratively"));
    assert!(prompt.contains("Executive summary (the only output)"));
    assert!(prompt.contains("No references, URLs, or appendices"));
}
