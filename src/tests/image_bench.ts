/**
 * Image Resizing Benchmark Utility
 *
 * This script measures:
 * 1. Total Latency: Time from start to completion.
 * 2. Main Thread Block Time: Estimated duration the UI thread is busy.
 */

export async function runBenchmark(resizeFn: (base64: string, mime: string, width: number) => Promise<string>) {
  console.log("Starting Benchmark...");

  // Generate a 4K image (3840x2160)
  const canvas = document.createElement('canvas');
  canvas.width = 3840;
  canvas.height = 2160;
  const ctx = canvas.getContext('2d')!;
  ctx.fillStyle = 'red';
  ctx.fillRect(0, 0, 3840, 2160);
  const base64 = canvas.toDataURL('image/png').split(',')[1];

  const iterations = 5;
  let totalTime = 0;
  let totalBlockTime = 0;

  for (let i = 0; i < iterations; i++) {
    console.log(`Iteration ${i + 1}...`);

    let blockedTime = 0;
    let isFinished = false;

    // Monitor main thread block time using requestAnimationFrame
    const monitorBlock = () => {
      let lastTime = performance.now();
      const check = () => {
        if (isFinished) return;
        const now = performance.now();
        const delta = now - lastTime;
        if (delta > 20) { // If a frame takes longer than 20ms, count it as blocked
          blockedTime += (delta - 16.6); // Subtract ideal frame time
        }
        lastTime = now;
        requestAnimationFrame(check);
      };
      requestAnimationFrame(check);
    };

    monitorBlock();

    const start = performance.now();
    await resizeFn(base64, 'image/png', 1024);
    const end = performance.now();

    isFinished = true;
    totalTime += (end - start);
    totalBlockTime += blockedTime;

    console.log(`  Latency: ${(end - start).toFixed(2)}ms`);
    console.log(`  Block Time: ${blockedTime.toFixed(2)}ms`);
  }

  console.log("--- Results ---");
  console.log(`Avg Latency: ${(totalTime / iterations).toFixed(2)}ms`);
  console.log(`Avg Block Time: ${(totalBlockTime / iterations).toFixed(2)}ms`);
}
