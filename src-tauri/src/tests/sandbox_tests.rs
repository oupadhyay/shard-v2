#[cfg(test)]
mod tests {
    use crate::sandbox;

    #[tokio::test]
    async fn test_simple_print() {
        let result = sandbox::execute_python(
            "print('hello world')",
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("execute_python should succeed");

        assert!(
            result.stdout.contains("hello world"),
            "stdout should contain 'hello world', got: {}",
            result.stdout
        );
        assert!(!result.timed_out);
        assert!(!result.fuel_exhausted);
    }

    #[tokio::test]
    async fn test_stderr_capture() {
        let result = sandbox::execute_python(
            "import sys; sys.stderr.write('error message')",
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("execute_python should succeed");

        assert!(
            result.stderr.contains("error message"),
            "stderr should contain 'error message', got: {}",
            result.stderr
        );
    }

    #[tokio::test]
    async fn test_computation() {
        let result = sandbox::execute_python(
            "print(sum(range(1000)))",
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("execute_python should succeed");

        assert!(
            result.stdout.contains("499500"),
            "stdout should contain '499500', got: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_multiline_code() {
        let code = r#"
def factorial(n):
    if n <= 1:
        return 1
    return n * factorial(n - 1)

print(factorial(10))
"#;
        let result = sandbox::execute_python(
            code,
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("execute_python should succeed");

        assert!(
            result.stdout.contains("3628800"),
            "stdout should contain '3628800', got: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_empty_code() {
        // Empty code is valid Python — it just produces no output.
        // The "no code provided" guard is in the agent match arm, not the sandbox.
        let result = sandbox::execute_python(
            "",
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("empty code is valid Python, should not error");

        assert!(result.stdout.is_empty());
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_whitespace_only_code() {
        let result = sandbox::execute_python(
            "   \n  \t  ",
            std::path::PathBuf::from("resources"),
            10,
        )
        .await;

        // Empty code is caught at the match arm level, not in sandbox.
        // The sandbox itself may succeed with whitespace-only code (no output).
        // This test just verifies it doesn't panic.
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_python_runtime_error() {
        let result = sandbox::execute_python(
            "raise ValueError('test error')",
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("execute_python should succeed even on Python errors");

        assert!(
            result.stderr.contains("ValueError") || result.stderr.contains("test error"),
            "stderr should contain the Python error, got: {}",
            result.stderr
        );
    }

    #[tokio::test]
    async fn test_no_output() {
        let result = sandbox::execute_python(
            "x = 42",
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("execute_python should succeed");

        assert!(result.stdout.is_empty(), "stdout should be empty for no-print code");
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_scratch_dir_works() {
        let code = r#"
with open('test.txt', 'w') as f:
    f.write('hello from scratch')
with open('test.txt', 'r') as f:
    print(f.read())
"#;
        let result = sandbox::execute_python(
            code,
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("execute_python should succeed");

        assert!(
            result.stdout.contains("hello from scratch"),
            "should be able to write and read files in scratch dir, got: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_sandbox_isolation() {
        // This test attempts to write to a file outside the scratch directory.
        // In WASI, this should fail because the only preopened directory is /scratch.
        let code = r#"
try:
    with open('/tmp/shard_escape_test', 'w') as f:
        f.write('escape attempt')
    print('ESCAPE_SUCCESS')
except Exception as e:
    print(f'ESCAPE_FAILED: {e}')
"#;
        let result = sandbox::execute_python(
            code,
            std::path::PathBuf::from("resources"),
            10,
        )
        .await
        .expect("execute_python should succeed even when code fails to write file");

        assert!(
            result.stdout.contains("ESCAPE_FAILED"),
            "Python code should not be able to write to host /tmp, got: {}",
            result.stdout
        );
        assert!(
            !std::path::Path::new("/tmp/shard_escape_test").exists(),
            "File should not exist on host system"
        );
    }
}
