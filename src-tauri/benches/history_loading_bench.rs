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
        });
    }

    let mut group = c.benchmark_group("history_loading");

    group.bench_function("serialize_100_messages", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&history)).unwrap();
            black_box(json);
        })
    });

    group.finish();
}

criterion_group!(benches, bench_history_serialization);
criterion_main!(benches);
