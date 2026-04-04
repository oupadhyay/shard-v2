use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use shard_lib::agent::{
    construct_gemini_messages, ChatMessage, FunctionCall, ToolCall,
};

/// Build a synthetic chat history with interleaved user, assistant (with tool
/// calls), and tool-response messages.
fn build_history(tool_rounds: usize) -> Vec<ChatMessage> {
    let mut history = Vec::with_capacity(1 + tool_rounds * 3);

    // Initial user message
    history.push(ChatMessage {
        role: "user".into(),
        content: Some("Hello, what's the weather?".into()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
        images: None,
        is_cron: None,
    });

    for i in 0..tool_rounds {
        let call_id = format!("call_{i}");
        let func_name = format!("tool_{i}");

        // Assistant with a tool call
        history.push(ChatMessage {
            role: "assistant".into(),
            content: Some(format!("Let me check {func_name}...")),
            reasoning: None,
            tool_calls: Some(vec![ToolCall {
                id: call_id.clone(),
                tool_type: "function".into(),
                function: FunctionCall {
                    name: func_name,
                    arguments: r#"{"q":"test"}"#.into(),
                },
                thought_signature: None,
            }]),
            tool_call_id: None,
            images: None,
            is_cron: None,
        });

        // Tool response
        history.push(ChatMessage {
            role: "tool".into(),
            content: Some(format!(r#"{{"result": "response_{i}"}}"#)),
            reasoning: None,
            tool_calls: None,
            tool_call_id: Some(call_id),
            images: None,
            is_cron: None,
        });

        // Assistant follow-up
        history.push(ChatMessage {
            role: "assistant".into(),
            content: Some(format!("Here is the result for round {i}.")),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
            is_cron: None,
        });
    }

    history
}

fn bench_construct_gemini_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("construct_gemini_messages");

    for tool_rounds in [10, 50, 200, 500] {
        let history = build_history(tool_rounds);
        let msg_count = history.len();

        group.bench_with_input(
            BenchmarkId::new("hashmap_lookup", format!("{msg_count}_msgs_{tool_rounds}_tools")),
            &history,
            |b, hist| {
                b.iter(|| {
                    let result = construct_gemini_messages(black_box(hist));
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_construct_gemini_messages);
criterion_main!(benches);
