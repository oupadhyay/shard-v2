use criterion::{black_box, criterion_group, criterion_main, Criterion};
use shard_lib::agent::ChatMessage;

fn bench_history_serialization(c: &mut Criterion) {
    let mut history = Vec::new();
    for i in 0..100 {
        history.push(ChatMessage {
            role: "assistant".to_string(),
            content: Some(format!(
                "This is message number {}. It contains a moderate amount of text to simulate a typical chat history entry. \
                Rendering this in the frontend used to cause layout thrashing due to repeated scrollHeight access.",
                i
            )),
            reasoning: Some(format!("Thinking process for message {}. Reasoning can often be quite long.", i)),
            tool_calls: None,
            tool_call_id: None,
            images: None,
            is_cron: None,
        });
    }

    let mut group = c.benchmark_group("history_serialization");
    group.noise_threshold(0.15); // Accommodate standard string allocation and Serde variance

    group.bench_function("serialize_100_messages", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&history)).unwrap();
            black_box(json);
        })
    });

    group.finish();
}

fn bench_session_transcript_formatting(c: &mut Criterion) {
    use shard_lib::sessions::format_transcript;
    let mut history = Vec::new();
    for i in 0..100 {
        history.push(ChatMessage {
            role: if i % 2 == 0 { "user".to_string() } else { "model".to_string() },
            content: Some(format!("Message content with some normal text {}", i)),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
            is_cron: None,
        });
    }

    let mut group = c.benchmark_group("history_serialization_format");
    group.noise_threshold(0.15);
    group.bench_function("format_100_messages_transcript", |b| {
        b.iter(|| {
            let formatted = format_transcript(black_box(&history));
            black_box(formatted);
        })
    });
    group.finish();
}

criterion_group!(benches, bench_history_serialization, bench_session_transcript_formatting);
criterion_main!(benches);
