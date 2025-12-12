# CEREBRAS.md

Cerebras gives you OpenAI‑compatible speed with wafer‑scale swagger. Swap your base URL, keep your muscle memory, and get frontier‑grade reasoning at GPU‑beating throughput.

- Base URL: <https://api.cerebras.ai/v1>
- Model: gpt-oss-120b
- Auth: Bearer CEREBRAS_API_KEY (.env)

## Quick Start (Rust)

Add deps:

```toml
# Cargo.toml
[dependencies]
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
futures-util = "0.3"
```

Basic chat:

```rust
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("CEREBRAS_API_KEY")?;
    let client = reqwest::Client::new();

    let body = json!({
        "model": "gpt-oss-120b",
        "messages": [
            {"role": "system", "content": "You are a concise technical assistant."},
            {"role": "user", "content": "Explain SIMD briefly."}
        ],
        "reasoning_effort": "medium" // low | medium | high for gpt-oss-120b
    });

    let resp = client
        .post("https://api.cerebras.ai/v1/chat/completions")
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, "rust-reqwest/0.12")
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;

    println!("{}", serde_json::to_string_pretty(&resp)?);
    Ok(())
}
```

## Streaming (SSE)

Set `stream: true` and read `text/event-stream` lines prefixed with `data:` until `[DONE]`.

```rust
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, ACCEPT, USER_AGENT};
use serde_json::Value;
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("CEREBRAS_API_KEY")?;
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": "gpt-oss-120b",
        "messages": [
            {"role": "system", "content": "Be pithy."},
            {"role": "user", "content": "Summarize fast inference benefits."}
        ],
        "stream": true,
        "reasoning_effort": "medium"
    });

    let mut resp = client
        .post("https://api.cerebras.ai/v1/chat/completions")
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "text/event-stream")
        .header(USER_AGENT, "rust-reqwest/0.12")
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    let mut buf = String::new();
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let s = String::from_utf8_lossy(&chunk?);
        buf.push_str(&s);

        while let Some(nl) = buf.find('\n') {
            let line = buf.drain(..=nl).collect::<String>();
            if let Some(payload) = line.strip_prefix("data: ") {
                let payload = payload.trim();
                if payload == "[DONE]" {
                    eprintln!("\n-- stream complete --");
                    return Ok(());
                }
                if let Ok(v) = serde_json::from_str::<Value>(payload) {
                    if let Some(delta) = v["choices"][0]["delta"]["content"].as_str() {
                        print!("{}", delta);
                    }
                }
            }
        }
    }
    Ok(())
}
```

## Tool Calls (Function Calling)

- Define tools as JSON Schema.
- When the assistant returns `tool_calls`, execute locally.
- Append a `role: "tool"` message with `tool_call_id`.
- Call chat again until you get a normal assistant message.

```rust
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{json, Value};

async fn chat_once(api_key: &str, messages: &Vec<Value>, tools: &Value) -> anyhow::Result<Value> {
    let client = reqwest::Client::new();
    let body = json!({
        "model": "gpt-oss-120b",
        "messages": messages,
        "tools": tools,
        "parallel_tool_calls": false,
        "reasoning_effort": "medium"
    });
    let resp = client
        .post("https://api.cerebras.ai/v1/chat/completions")
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, "rust-reqwest/0.12")
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(resp)
}

fn calculate(expression: &str) -> String {
    match expression {
        "15 * 7" => "105".to_string(),
        _ => "0".to_string()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("CEREBRAS_API_KEY")?;

    let tools = json!([
      {
        "type": "function",
        "function": {
          "name": "calculate",
          "strict": true,
          "description": "Compute arithmetic expressions.",
          "parameters": {
            "type": "object",
            "properties": {
              "expression": { "type": "string", "description": "Math expression" }
            },
            "required": ["expression"],
            "additionalProperties": false
          }
        }
      }
    ]);

    let mut messages = vec![
        json!({"role": "system", "content": "Use the calculator tool for math."}),
        json!({"role": "user", "content": "What is 15 * 7?"})
    ];

    loop {
        let resp = chat_once(&api_key, &messages, &tools).await?;
        let msg = &resp["choices"][0]["message"];

        if let Some(tc) = msg.get("tool_calls") {
            messages.push(msg.clone()); // save assistant turn exactly
            let call = &tc[0]["function"];
            let args = call["arguments"].as_str().unwrap_or("{}");
            let parsed: Value = serde_json::from_str(args)?;
            let expr = parsed["expression"].as_str().unwrap_or("");

            let result = calculate(expr);

            messages.push(json!({
                "role": "tool",
                "tool_call_id": tc[0]["id"],
                "content": serde_json::to_string(&result)?
            }));
            continue; // ask the model to continue with the new tool result
        } else {
            println!("Assistant: {}", msg["content"].as_str().unwrap_or(""));
            break;
        }
    }
    Ok(())
}
```

## Model Notes

- gpt‑oss‑120b supports reasoning and streaming; control depth via `reasoning_effort ∈ {low, medium, high}`.
- System messages act at “developer level” for this model, so they carry more weight.
- Endpoint: `POST /v1/chat/completions`; pass `Authorization: Bearer <CEREBRAS_API_KEY>`.
- Streaming emits SSE with `choices[].delta.content` chunks and ends with `data: [DONE]`.

## Operational Tips

- Set a real `User-Agent` to avoid CloudFront gripes.
- For structured outputs, use `response_format: { type: "json_schema", json_schema: {...} }` (non‑streaming for strict JSON).
- Keep tool schemas tight (`strict: true`, `additionalProperties: false`) to reduce argument drift.

## OpenAI GPT OSS

Model ID: gpt-oss-120b
[Model card](https://openai.com/index/gpt-oss-model-card/)

### Capabilities

- **Reasoning:** Efficient across science, math, and coding.
- Streaming, Structured Outputs, Tool Calling.
- Suitable for real-time coding assistance, large-document Q&A/summarization, agentic research, and regulated on‑prem workloads.

### Performance

- Speed: ~3000 tokens/sec.
- Context: Free 65k, Paid 131k.
- Max output: Free 32k, Paid 40k.

### Pricing

- Input: $0.35 per million tokens.
- Output: $0.75 per million tokens.
- For volume discounts and enterprise features: see [pricing page](https://www.cerebras.ai/pricing).

### API

- Endpoint: /v1/chat/completions.
- OpenAI compatibility: system role maps to developer-level instructions. See [guide](https://inference-docs.cerebras.ai/resources/openai#developer-role).

### Parameters & Notes

- reasoning_effort (default: medium). See [reasoning guide](https://inference-docs.cerebras.ai/capabilities/reasoning).
- Caution: Setting min_tokens may cause EOS tokens and parser failures.
- Model may attempt non-approved tool calls; reprompt with “you’re hallucinating a tool call” to correct behavior.

### Rate Limits

- Free: 30 requests/min, 60k input tokens/min, 1M daily tokens.
- Developer: 1000 requests/min, 1M input tokens/min.
