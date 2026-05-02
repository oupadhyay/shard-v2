use crate::prompts::get_research_system_prompt;

#[test]
fn test_research_prompt_integrity() {
    let prompt = get_research_system_prompt(None, None);
    assert!(prompt.contains("Deep Research agent"));
    assert!(prompt.contains("Produce an initial research plan"));
    assert!(prompt.contains("Execute iteratively"));
    assert!(prompt.contains("Executive summary"));
    // Citations are REQUIRED — formatted as inline markdown links — and the
    // model must NOT emit a trailing Sources/References section.
    assert!(prompt.contains("inline markdown link"));
    assert!(prompt.contains("[domain or short title](https://full-url)"));
    assert!(
        prompt.contains("No trailing \"Sources\" or \"References\" section"),
        "research prompt must explicitly forbid a trailing sources list"
    );
    assert!(
        !prompt.contains("No references, URLs, or appendices"),
        "research prompt must NOT contain the old no-citations rule"
    );
    assert!(
        !prompt.contains("do not include citations"),
        "research prompt must NOT forbid citations"
    );
}
