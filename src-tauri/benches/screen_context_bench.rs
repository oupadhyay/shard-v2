use criterion::{black_box, criterion_group, criterion_main, Criterion};
use image::{DynamicImage, RgbImage};
use std::time::Duration;

/// Generate a synthetic test image with embedded text patterns
/// Uses simple patterns that exercise OCR detection without requiring real text
fn generate_test_image(width: u32, height: u32) -> DynamicImage {
    let mut img = RgbImage::new(width, height);

    // Fill with white background
    for pixel in img.pixels_mut() {
        *pixel = image::Rgb([255, 255, 255]);
    }

    // Add horizontal black lines (simulates text lines)
    for y in (50..height).step_by(30) {
        if y + 10 < height {
            for x in 50..(width - 50).min(width) {
                for dy in 0..8 {
                    if y + dy < height {
                        img.put_pixel(x, y + dy, image::Rgb([0, 0, 0]));
                    }
                }
            }
        }
    }

    DynamicImage::ImageRgb8(img)
}

/// Benchmark local OCR engine initialization
/// Note: This requires the bundled OCR models to be present
fn bench_ocr_image_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("screen_context");
    group.sample_size(10);

    // Benchmark image generation at various resolutions
    let resolutions = [
        ("720p", 1280, 720),
        ("1080p", 1920, 1080),
        ("1440p", 2560, 1440),
        ("4K", 3840, 2160),
    ];

    for (name, width, height) in resolutions {
        group.bench_function(format!("generate_image_{}", name), |b| {
            b.iter(|| generate_test_image(black_box(width), black_box(height)))
        });
    }

    group.finish();
}

/// Benchmark image encoding (JPEG compression used in screen capture)
fn bench_image_encoding(c: &mut Criterion) {
    let mut group = c.benchmark_group("screen_context");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(5));

    let resolutions = [
        ("720p", 1280, 720),
        ("1080p", 1920, 1080),
        ("4K", 3840, 2160),
    ];

    for (name, width, height) in resolutions {
        let img = generate_test_image(width, height);
        let rgb_img = img.to_rgb8();

        group.bench_function(format!("encode_jpeg_{}", name), |b| {
            b.iter(|| {
                let mut jpeg_data: Vec<u8> = Vec::new();
                let mut cursor = std::io::Cursor::new(&mut jpeg_data);
                let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 75);
                encoder.encode_image(black_box(&rgb_img)).unwrap();
                jpeg_data
            })
        });
    }

    group.finish();
}

/// Benchmark image resize operations (used to reduce image size before Vision LLM)
/// Compares Nearest (fast, minor aliasing) vs Triangle (smoother, slower) filters
fn bench_image_resize(c: &mut Criterion) {
    let mut group = c.benchmark_group("screen_context");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(5));

    // Test resizing from various resolutions to 1280px width
    let resolutions = [
        ("1440p", 2560, 1440),
        ("4K", 3840, 2160),
    ];

    let filters = [
        ("nearest", image::imageops::FilterType::Nearest),
        ("triangle", image::imageops::FilterType::Triangle),
    ];

    for (res_name, width, height) in resolutions {
        let source_img = generate_test_image(width, height);
        let rgba_img = source_img.to_rgba8();

        // Pre-compute target dimensions outside b.iter() to isolate resize cost
        let max_width = 1280u32;
        let scale = max_width as f32 / rgba_img.width() as f32;
        let new_height = (rgba_img.height() as f32 * scale) as u32;

        for (filter_name, filter_type) in &filters {
            group.bench_function(format!("resize_{}_{}_to_1280w", res_name, filter_name), |b| {
                b.iter(|| {
                    image::imageops::resize(
                        black_box(&rgba_img),
                        max_width,
                        new_height,
                        *filter_type,
                    )
                })
            });
        }
    }

    group.finish();
}

/// Benchmark JSON parsing for analysis responses
fn bench_json_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("screen_context");
    group.sample_size(100);

    let simple_json = r#"{"summary": "User browsing web", "suggestions": ["Summarize this page", "Extract key points"]}"#;
    let markdown_wrapped = "```json\n{\"summary\": \"test\", \"suggestions\": [\"a\", \"b\", \"c\"]}\n```";
    let complex_json = r#"{"summary": "User working in VS Code with multiple files open, appears to be debugging Rust code with terminal output visible", "suggestions": ["Help me understand this error message", "Suggest fixes for this code", "Explain the Rust ownership model"]}"#;

    group.bench_function("parse_json_simple", |b| {
        b.iter(|| {
            let json_str = black_box(simple_json)
                .trim()
                .trim_start_matches("```json")
                .trim_end_matches("```")
                .trim();
            serde_json::from_str::<serde_json::Value>(json_str)
        })
    });

    group.bench_function("parse_json_markdown", |b| {
        b.iter(|| {
            let json_str = black_box(markdown_wrapped)
                .trim()
                .trim_start_matches("```json")
                .trim_end_matches("```")
                .trim();
            serde_json::from_str::<serde_json::Value>(json_str)
        })
    });

    group.bench_function("parse_json_complex", |b| {
        b.iter(|| {
            let json_str = black_box(complex_json)
                .trim()
                .trim_start_matches("```json")
                .trim_end_matches("```")
                .trim();
            serde_json::from_str::<serde_json::Value>(json_str)
        })
    });

    group.finish();
}

fn configure_criterion() -> Criterion {
    Criterion::default()
        .noise_threshold(0.05)
        .significance_level(0.01)
        .measurement_time(Duration::from_secs(3))
}

criterion_group! {
    name = benches;
    config = configure_criterion();
    targets = bench_ocr_image_creation, bench_image_encoding, bench_image_resize, bench_json_parsing
}
criterion_main!(benches);
