use anyhow::Result;
use std::path::Path;

pub fn analyze_log(content: &str) -> String {
    let mut findings = Vec::new();

    let error_patterns = [
        ("panic", "Rust panic detected"),
        ("segmentation fault", "Segmentation fault"),
        ("SIGSEGV", "Signal SIGSEGV"),
        ("Error:", "Generic error"),
        ("error:", "Generic error"),
        ("Exception", "Exception thrown"),
        ("FATAL", "Fatal error"),
        ("assertion failed", "Assertion failure"),
        ("stack trace", "Stack trace available"),
        ("undefined reference", "Linker error"),
        ("cannot find", "Missing dependency or file"),
    ];

    for (pattern, description) in &error_patterns {
        if content.to_lowercase().contains(&pattern.to_lowercase()) {
            findings.push(format!("- {description} (matched: {pattern})"));
        }
    }

    let lines: Vec<&str> = content.lines().collect();
    let error_lines: Vec<String> = lines
        .iter()
        .filter(|l| {
            let lower = l.to_lowercase();
            lower.contains("error") || lower.contains("panic") || lower.contains("fatal")
        })
        .take(10)
        .map(|l| format!("  > {l}"))
        .collect();

    let mut report = String::from("=== Log Analysis ===\n\n");
    if findings.is_empty() {
        report.push_str("No common error patterns detected.\n");
    } else {
        report.push_str("Findings:\n");
        report.push_str(&findings.join("\n"));
        report.push('\n');
    }

    if !error_lines.is_empty() {
        report.push_str("\nRelevant lines:\n");
        report.push_str(&error_lines.join("\n"));
        report.push('\n');
    }

    report.push_str("\nSuggested actions:\n");
    report.push_str("1. Check the first error line in the stack trace\n");
    report.push_str("2. Reproduce with minimal test case\n");
    report.push_str("3. Run with debug symbols enabled\n");
    report.push_str("4. Use `forge debug start` for interactive debugging\n");

    report
}

pub async fn start_debugger(debugger: &str, program: &Path, args: &[String]) -> Result<()> {
    let mut cmd = tokio::process::Command::new(debugger);
    cmd.arg("--");
    cmd.arg(program);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    let status = cmd.status().await?;
    if !status.success() {
        anyhow::bail!("debugger exited with {status}");
    }
    Ok(())
}
