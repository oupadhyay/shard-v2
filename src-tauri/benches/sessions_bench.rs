use criterion::{black_box, criterion_group, criterion_main, Criterion};
use shard_lib::sessions::{format_transcript, parse_llm_response, sanitize_slug};
use shard_lib::agent::ChatMessage;

fn bench_sanitize_slug(c: &mut Criterion) {
    let mut group = c.benchmark_group("sessions_sanitize_slug");
    group.noise_threshold(0.15); // Adjust noise threshold for nanosecond string operations

    let text_clean = "simple slug without symbols";
    group.bench_function("clean_text", |b| b.iter(|| sanitize_slug(black_box(text_clean))));

    let text_dirty = "A Very, VERY!! Dirty ~Slug~ With @Symbols & Stuff 123";
    group.bench_function("dirty_text", |b| b.iter(|| sanitize_slug(black_box(text_dirty))));

    group.finish();
}

fn bench_parse_llm_response(c: &mut Criterion) {
    let mut group = c.benchmark_group("sessions_parse_llm_response");
    group.noise_threshold(0.15); // Higher allowed variance for fast Serde JSON allocs

    let response = "SLUG: highly-optimized-bench\nSUMMARY: This is a test of the fast parsing logic.\nIt spans multiple lines.\nAnd contains some insight.";
    group.bench_function("standard_response", |b| {
        b.iter(|| parse_llm_response(black_box(response)))
    });

    let empty_response = "";
    group.bench_function("empty_response", |b| {
        b.iter(|| parse_llm_response(black_box(empty_response)))
    });

    group.finish();
}

fn bench_format_transcript(c: &mut Criterion) {
    let mut group = c.benchmark_group("sessions_format_transcript");
    group.noise_threshold(0.15);

    let history = vec![
        ChatMessage {
            role: "user".to_string(),
            content: Some("Hello, I need help with Rust benchmarking.".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
            is_cron: None,
        },
        ChatMessage {
            role: "model".to_string(),
            content: Some("Sure! Let's talk about Criterion and how to structure your benchmark loops.".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
            is_cron: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: Some("Great, how do I setup a group?".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
            is_cron: None,
        },
    ];

    group.bench_function("short_conversation", |b| b.iter(|| format_transcript(black_box(&history))));

    group.finish();
}

criterion_group!(benches, bench_sanitize_slug, bench_parse_llm_response, bench_format_transcript);
criterion_main!(benches);
