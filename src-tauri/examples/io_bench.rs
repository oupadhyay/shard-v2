use std::time::Instant;

#[tokio::main]
async fn main() {
    let size = 2 * 1024 * 1024; // 2MB
    let data = vec![0u8; size];
    // Use tempfile so each run gets a unique path and the file is auto-removed on drop.
    let temp_file = tempfile::NamedTempFile::new().expect("create tempfile");
    let temp_path = temp_file.path().to_path_buf();
    std::fs::write(&temp_path, &data).unwrap();

    println!("Benchmarking 2MB file read...");

    // Baseline: std::fs::read
    let start = Instant::now();
    for _ in 0..100 {
        let result = std::fs::read(&temp_path).unwrap();
        std::hint::black_box(result);
    }
    let std_duration = start.elapsed() / 100;
    println!("std::fs::read average: {:?}", std_duration);

    // Tokio: tokio::fs::read
    let start = Instant::now();
    for _ in 0..100 {
        let result = tokio::fs::read(&temp_path).await.unwrap();
        std::hint::black_box(result);
    }
    let tokio_duration = start.elapsed() / 100;
    println!("tokio::fs::read average: {:?}", tokio_duration);

    // temp_file dropped here automatically removes the file.
    drop(temp_file);
}
