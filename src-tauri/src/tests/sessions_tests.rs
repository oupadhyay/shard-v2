use crate::sessions::{parse_llm_response, sanitize_slug};

#[test]
fn test_sanitize_slug() {
    assert_eq!(sanitize_slug("  API Design Discussion! "), "api-design-discussion");
    assert_eq!(sanitize_slug("rust-tauri-sqlite"), "rust-tauri-sqlite");
    assert_eq!(sanitize_slug("  just a slug..."), "just-a-slug");
}

#[test]
fn test_parse_llm_response() {
    let response = "SLUG: ui-redesign-chat\nSUMMARY: Discussed the new dark mode theme.\nAnd also buttons.";
    let (slug, summary) = parse_llm_response(response);
    assert_eq!(slug, "ui-redesign-chat");
    assert_eq!(summary, "Discussed the new dark mode theme. And also buttons.");

    // Edge case: missing everything
    let response_bad = "I'm not sure what you want.";
    let (slug2, summary2) = parse_llm_response(response_bad);
    assert_eq!(slug2, "session");
    assert_eq!(summary2, "No summary generated.");

    // Edge case: only slug
    let response_slug_only = "SLUG: only-slug";
    let (slug3, summary3) = parse_llm_response(response_slug_only);
    assert_eq!(slug3, "only-slug");
    assert_eq!(summary3, "No summary generated.");

    // Edge case: only summary
    let response_summary_only = "SUMMARY: Only a summary here.";
    let (slug4, summary4) = parse_llm_response(response_summary_only);
    assert_eq!(slug4, "session");
    assert_eq!(summary4, "Only a summary here.");

    // Edge case: empty lines in summary
    let response_empty_lines = "SLUG: test-summary\nSUMMARY: First line.\n\nSecond line.";
    let (slug5, summary5) = parse_llm_response(response_empty_lines);
    assert_eq!(slug5, "test-summary");
    assert_eq!(summary5, "First line. Second line.");
}
