use criterion::{black_box, criterion_group, criterion_main, Criterion};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Sample base64-encoded image (1x1 red pixel PNG - tests color recognition)
/// This is a 1x1 red (#FF0000) pixel encoded as PNG
const SAMPLE_IMAGE_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

#[derive(Serialize)]
struct OpenAIVisionRequest {
    model: String,
    messages: Vec<VisionMessage>,
    max_tokens: u32,
}

#[derive(Serialize)]
struct VisionMessage {
    role: String,
    content: Vec<VisionContent>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum VisionContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlPayload },
}

#[derive(Serialize)]
struct ImageUrlPayload {
    url: String,
}

#[derive(Deserialize, Clone)]
struct OpenAIResponse {
    choices: Option<Vec<OpenAIChoice>>,
    error: Option<OpenAIError>,
}

#[derive(Deserialize, Clone)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize, Clone)]
struct OpenAIMessage {
    content: Option<String>,
}

#[derive(Deserialize, Clone)]
struct OpenAIError {
    message: String,
}

/// Call OpenRouter Vision API with gemma-3-12b
async fn call_vision_openrouter(
    client: &Client,
    image_base64: &str,
    api_key: &str,
) -> Result<String, String> {
    let request = OpenAIVisionRequest {
        model: "google/gemma-3-12b-it:free".to_string(),
        messages: vec![VisionMessage {
            role: "user".to_string(),
            content: vec![
                VisionContent::Text {
                    text: "What color is this pixel?".to_string(),
                },
                VisionContent::ImageUrl {
                    image_url: ImageUrlPayload {
                        url: format!("data:image/png;base64,{}", image_base64),
                    },
                },
            ],
        }],
        max_tokens: 100,
    };

    let response = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let result: OpenAIResponse = response.json().await.map_err(|e| e.to_string())?;

    if let Some(error) = result.error {
        return Err(error.message);
    }

    result
        .choices
        .and_then(|c| c.first().cloned())
        .and_then(|c| c.message.content)
        .ok_or_else(|| "No content in response".to_string())
}

/// Call Groq Vision API with Llama 4 Scout
async fn call_vision_groq(
    client: &Client,
    image_base64: &str,
    api_key: &str,
) -> Result<String, String> {
    let request = OpenAIVisionRequest {
        model: "meta-llama/llama-4-scout-17b-16e-instruct".to_string(),
        messages: vec![VisionMessage {
            role: "user".to_string(),
            content: vec![
                VisionContent::Text {
                    text: "What color is this pixel?".to_string(),
                },
                VisionContent::ImageUrl {
                    image_url: ImageUrlPayload {
                        url: format!("data:image/png;base64,{}", image_base64),
                    },
                },
            ],
        }],
        max_tokens: 100,
    };

    let response = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let result: OpenAIResponse = response.json().await.map_err(|e| e.to_string())?;

    if let Some(error) = result.error {
        return Err(error.message);
    }

    result
        .choices
        .and_then(|c| c.first().cloned())
        .and_then(|c| c.message.content)
        .ok_or_else(|| "No content in response".to_string())
}

/// Benchmark OpenRouter Vision LLM (using gemma-3-12b to avoid quota saturation)
fn bench_openrouter_vision(c: &mut Criterion) {
    // Load .env file from project root (two levels up from src-tauri/)
    let env_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".env");

    if env_path.exists() {
        dotenvy::from_path(&env_path).ok();
    }

    let api_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("Skipping OpenRouter Vision benchmark: OPENROUTER_API_KEY not found in .env");
            return;
        }
    };

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = Client::new();

    c.bench_function("vision/openrouter_gemma3_12b", |b| {
        b.iter_custom(|iters| {
            let start = std::time::Instant::now();
            for _ in 0..iters {
                runtime.block_on(async {
                    let _ = call_vision_openrouter(
                        black_box(&client),
                        black_box(SAMPLE_IMAGE_BASE64),
                        black_box(&api_key),
                    )
                    .await;
                });
            }
            start.elapsed()
        })
    });
}

/// Benchmark Groq Vision LLM (Llama 4 Scout)
fn bench_groq_vision(c: &mut Criterion) {
    // Load .env file from project root (two levels up from src-tauri/)
    let env_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".env");

    if env_path.exists() {
        dotenvy::from_path(&env_path).ok();
    }

    let api_key = match std::env::var("GROQ_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("Skipping Groq Vision benchmark: GROQ_API_KEY not found in .env");
            return;
        }
    };

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = Client::new();

    c.bench_function("vision/groq_llama4_scout", |b| {
        b.iter_custom(|iters| {
            let start = std::time::Instant::now();
            for _ in 0..iters {
                runtime.block_on(async {
                    let _ = call_vision_groq(
                        black_box(&client),
                        black_box(SAMPLE_IMAGE_BASE64),
                        black_box(&api_key),
                    )
                    .await;
                });
            }
            start.elapsed()
        })
    });
}

fn configure_criterion() -> Criterion {
    Criterion::default()
        .noise_threshold(0.10) // Higher threshold for network variance
        .significance_level(0.05) // Less strict for API latency
        .measurement_time(Duration::from_secs(20)) // Longer measurement for API calls
        .sample_size(5) // Reduced from 10 due to long API latency
}

criterion_group! {
    name = benches;
    config = configure_criterion();
    targets = bench_openrouter_vision, bench_groq_vision
}
criterion_main!(benches);
