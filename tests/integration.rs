use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn create_temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("crappy-integ-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let src = dir.join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        dir.join("Cargo.toml"),
        r#"[package]
name = "test-crate"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    dir
}

#[test]
fn full_pipeline_on_simple_project() {
    let dir = create_temp_project("simple");

    fs::write(
        dir.join("src/lib.rs"),
        r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn uncovered_branchy(x: i32) -> &'static str {
    if x > 0 {
        if x > 100 {
            "big"
        } else {
            "small"
        }
    } else {
        "negative"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }
}
"#,
    )
    .unwrap();

    let output = Command::new("cargo")
        .args(["crappy", "--threshold", "30"])
        .current_dir(&dir)
        .output()
        .expect("cargo crappy should run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stderr.contains("functions with coverage data"),
        "should report coverage count: {stderr}"
    );
    assert!(
        stderr.contains("functions analyzed"),
        "should report complexity count: {stderr}"
    );

    assert!(
        stdout.contains("CRAP"),
        "output should have header: {stdout}"
    );
    assert!(stdout.contains("add"), "should list add function: {stdout}");
    assert!(
        stdout.contains("uncovered_branchy"),
        "should list uncovered_branchy: {stdout}"
    );

    assert!(
        stdout.contains("Total functions: 2"),
        "should have 2 functions: {stdout}"
    );
    assert!(
        stdout.contains("Functions above CRAP threshold (30):"),
        "should show threshold line: {stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn threshold_exit_code() {
    let dir = create_temp_project("exitcode");

    fs::write(
        dir.join("src/lib.rs"),
        r#"
pub fn simple() -> i32 { 42 }

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(super::simple(), 42);
    }
}
"#,
    )
    .unwrap();

    let output = Command::new("cargo")
        .args(["crappy", "--threshold", "100"])
        .current_dir(&dir)
        .output()
        .expect("cargo crappy should run");

    assert!(
        output.status.success(),
        "simple fully-covered function should pass threshold=100: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new("cargo")
        .args(["crappy", "--threshold", "0"])
        .current_dir(&dir)
        .output()
        .expect("cargo crappy should run");

    assert_eq!(
        output.status.code(),
        Some(1),
        "CRAP=1.0 should exceed threshold=0"
    );

    let _ = fs::remove_dir_all(&dir);
}
