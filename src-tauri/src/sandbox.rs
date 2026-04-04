use std::path::PathBuf;
use std::sync::OnceLock;
use wasmtime::*;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;

// ── Types ────────────────────────────────────────────────────────────────────

pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub fuel_exhausted: bool,
}

// ── Configuration ────────────────────────────────────────────────────────────

const FUEL_LIMIT: u64 = 10_000_000_000; // ~10-30s of CPU
const MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024; // 256 MB
const DEFAULT_TIMEOUT_SECS: u64 = 30;

// ── Public API ───────────────────────────────────────────────────────────────

/// Execute Python code inside the WASI sandbox.
///
/// - `code`: Python source to execute
/// - `resource_dir`: path to app resources (contains python.wasm)
/// - `timeout_secs`: max wall-clock seconds (default 30)
pub async fn execute_python(
    code: &str,
    resource_dir: PathBuf,
    timeout_secs: u64,
) -> Result<ExecutionResult, String> {
    let timeout = if timeout_secs == 0 {
        DEFAULT_TIMEOUT_SECS
    } else {
        timeout_secs
    };

    // Always use WASI sandbox for security (true isolation)
    execute_python_wasi(code, resource_dir, timeout).await
}


// ── WASI Sandbox ─────────────────────────────────────────────────────────────

/// Cached compiled Wasmtime module. Compilation takes 1-2s; cache it for reuse.
/// Engine and Module are Send + Sync, so OnceLock alone suffices.
static PYTHON_MODULE: OnceLock<(Engine, Module)> = OnceLock::new();

/// Load or return the cached (Engine, Module) pair.
/// Uses get() + set() instead of get_or_try_init (unstable).
/// Minor race on first call is harmless — only one value wins set().
fn get_or_compile_module(resource_dir: &PathBuf) -> Result<&'static (Engine, Module), String> {
    if let Some(cached) = PYTHON_MODULE.get() {
        return Ok(cached);
    }

    let wasm_path = resource_dir.join("python.wasm");
    if !wasm_path.exists() {
        return Err(format!(
            "python.wasm not found at {}. WASI fallback unavailable.",
            wasm_path.display()
        ));
    }

    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);

    let engine =
        Engine::new(&config).map_err(|e| format!("Failed to create Wasmtime engine: {}", e))?;
    let module = Module::from_file(&engine, &wasm_path)
        .map_err(|e| format!("Failed to compile python.wasm: {}", e))?;

    // Race-safe: if another thread already set it, our work is discarded
    let _ = PYTHON_MODULE.set((engine, module));
    PYTHON_MODULE
        .get()
        .ok_or_else(|| "Failed to cache compiled module".to_string())
}

/// Resource limiter for Wasmtime store — caps memory allocation.
struct SandboxLimiter;

impl ResourceLimiter for SandboxLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool> {
        Ok(desired <= MEMORY_LIMIT_BYTES)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool> {
        Ok(desired <= 100_000)
    }
}

/// Host state for the WASI preview1 store.
struct SandboxState {
    wasi: WasiP1Ctx,
    limiter: SandboxLimiter,
}

/// Execute Python code inside the Wasmtime WASI sandbox.
async fn execute_python_wasi(
    code: &str,
    resource_dir: PathBuf,
    timeout_secs: u64,
) -> Result<ExecutionResult, String> {
    let code = code.to_string();

    // Wasmtime execution is synchronous/CPU-bound — run on blocking thread pool
    tokio::task::spawn_blocking(move || {
        execute_python_wasi_sync(&code, &resource_dir, timeout_secs)
    })
    .await
    .map_err(|e| format!("Sandbox task panicked: {}", e))?
}

fn execute_python_wasi_sync(
    code: &str,
    resource_dir: &PathBuf,
    timeout_secs: u64,
) -> Result<ExecutionResult, String> {
    let (engine, module) = get_or_compile_module(resource_dir)?;

    // Create scratch directory
    let scratch = tempfile::tempdir().map_err(|e| format!("Failed to create temp dir: {}", e))?;
    let script_path = scratch.path().join("main.py");
    std::fs::write(&script_path, code).map_err(|e| format!("Failed to write script: {}", e))?;

    // Capture pipes
    let stdout_pipe = MemoryOutputPipe::new(1024 * 1024); // 1MB buffer
    let stderr_pipe = MemoryOutputPipe::new(256 * 1024); // 256KB buffer

    // Build WASI preview1 context
    let wasi_p1 = WasiCtxBuilder::new()
        .stdout(stdout_pipe.clone())
        .stderr(stderr_pipe.clone())
        .args(&["python", "/scratch/main.py"])
        .env("PYTHONPATH", "/scratch")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .preopened_dir(
            scratch.path(),
            "/scratch",
            wasmtime_wasi::DirPerms::all(),
            wasmtime_wasi::FilePerms::all(),
        )
        .map_err(|e| format!("Failed to preopen scratch dir: {}", e))?
        .build_p1();

    // Create store with resource limits
    let mut store = Store::new(
        engine,
        SandboxState {
            wasi: wasi_p1,
            limiter: SandboxLimiter,
        },
    );

    // Set resource limiter
    store.limiter(|state| &mut state.limiter);

    // Set fuel budget
    store
        .set_fuel(FUEL_LIMIT)
        .map_err(|e| format!("Failed to set fuel: {}", e))?;

    // Set epoch deadline for wall-clock timeout
    store.epoch_deadline_trap();
    store.set_epoch_deadline(1);

    // Spawn epoch incrementer thread for timeout
    let engine_clone = engine.clone();
    let timeout_handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(timeout_secs));
        engine_clone.increment_epoch();
    });

    // Link WASI preview1 and instantiate
    let mut linker = Linker::<SandboxState>::new(engine);
    preview1::add_to_linker_sync(&mut linker, |state: &mut SandboxState| &mut state.wasi)
        .map_err(|e| format!("Failed to link WASI: {}", e))?;

    let instance = linker
        .instantiate(&mut store, module)
        .map_err(|e| format!("Failed to instantiate module: {}", e))?;

    // Get and call _start (WASI entry point)
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .map_err(|e| format!("Failed to find _start export: {}", e))?;

    let call_result = start.call(&mut store, ());

    // Determine outcome
    let mut timed_out = false;
    let mut fuel_exhausted = false;

    match call_result {
        Ok(()) => {}
        Err(err) => {
            let err_str = err.to_string();
            if err_str.contains("epoch") || err_str.contains("interrupt") {
                timed_out = true;
            } else if err_str.contains("fuel") {
                fuel_exhausted = true;
            } else if err.downcast_ref::<wasmtime_wasi::I32Exit>().is_some() {
                // Normal Python exit (e.g., sys.exit(0)) — not an error
            } else {
                // Other trap (e.g., memory OOB)
                let stdout =
                    String::from_utf8_lossy(&stdout_pipe.try_into_inner().unwrap_or_default())
                        .to_string();
                let mut stderr =
                    String::from_utf8_lossy(&stderr_pipe.try_into_inner().unwrap_or_default())
                        .to_string();
                stderr.push_str(&format!("\nWasm trap: {}", err_str));

                let _ = timeout_handle;
                return Ok(ExecutionResult {
                    stdout,
                    stderr,
                    timed_out: false,
                    fuel_exhausted: false,
                });
            }
        }
    }

    // Extract captured output
    let stdout =
        String::from_utf8_lossy(&stdout_pipe.try_into_inner().unwrap_or_default()).to_string();
    let stderr =
        String::from_utf8_lossy(&stderr_pipe.try_into_inner().unwrap_or_default()).to_string();

    // Clean up timeout thread (it'll finish on its own, harmless)
    let _ = timeout_handle;

    Ok(ExecutionResult {
        stdout,
        stderr,
        timed_out,
        fuel_exhausted,
    })
}
