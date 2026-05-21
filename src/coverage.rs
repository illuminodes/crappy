use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Error;

pub struct FunctionCoverage {
    pub file: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub line_coverage_pct: f64,
}

// --- cargo test --message-format=json structs ---

bourne::from_json! {
    #[bourne(deny_unknown_fields = false)]
    struct CargoArtifact {
        reason: String,
        #[bourne(default)]
        executable: Option<String>,
        #[bourne(default)]
        profile: Option<CargoProfile>,
    }
}

bourne::from_json! {
    #[bourne(deny_unknown_fields = false)]
    struct CargoProfile {
        #[bourne(default)]
        test: bool,
    }
}

// --- llvm-cov export JSON structs ---

bourne::from_json! {
    #[bourne(deny_unknown_fields = false)]
    struct LlvmCovExport {
        data: Vec<LlvmCovData>,
    }
}

bourne::from_json! {
    #[bourne(deny_unknown_fields = false)]
    struct LlvmCovData {
        functions: Vec<LlvmCovFunction>,
    }
}

bourne::from_json! {
    #[bourne(deny_unknown_fields = false)]
    struct LlvmCovFunction {
        filenames: Vec<String>,
        regions: Vec<Vec<u64>>,
    }
}

pub(crate) struct LlvmTools {
    pub profdata: PathBuf,
    pub cov: PathBuf,
}

pub(crate) fn resolve_llvm_tools(sysroot: &str, host: &str) -> Result<LlvmTools, Error> {
    let bin_dir = PathBuf::from(sysroot)
        .join("lib")
        .join("rustlib")
        .join(host)
        .join("bin");

    let profdata = bin_dir.join("llvm-profdata");
    let cov = bin_dir.join("llvm-cov");

    if !profdata.exists() {
        return Err(Error::ToolNotFound {
            tool: profdata.display().to_string(),
        });
    }
    if !cov.exists() {
        return Err(Error::ToolNotFound {
            tool: cov.display().to_string(),
        });
    }

    Ok(LlvmTools { profdata, cov })
}

pub(crate) fn parse_rustc_host(version_output: &str) -> Option<String> {
    version_output
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(|s| s.to_string())
}

fn find_llvm_tools() -> Result<LlvmTools, Error> {
    let sysroot_out = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()?;
    let sysroot = String::from_utf8_lossy(&sysroot_out.stdout)
        .trim()
        .to_string();

    let version_out = Command::new("rustc").arg("-vV").output()?;
    let version_str = String::from_utf8_lossy(&version_out.stdout);
    let host = parse_rustc_host(&version_str).ok_or_else(|| Error::ToolNotFound {
        tool: "rustc host triple".into(),
    })?;

    resolve_llvm_tools(&sysroot, &host)
}

fn clean_profraw(crappy_dir: &Path) -> Result<(), Error> {
    if crappy_dir.exists() {
        for entry in fs::read_dir(crappy_dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "profraw") {
                fs::remove_file(&path)?;
            }
        }
    } else {
        fs::create_dir_all(crappy_dir)?;
    }
    Ok(())
}

pub(crate) fn find_test_binaries(stdout: &str) -> Vec<PathBuf> {
    let mut binaries = Vec::new();
    for line in stdout.lines() {
        if !line.starts_with('{') {
            continue;
        }
        let Ok(artifact) = bourne::parse_str::<CargoArtifact>(line) else {
            continue;
        };
        if artifact.reason != "compiler-artifact" {
            continue;
        }
        let is_test = artifact.profile.as_ref().is_some_and(|p| p.test);
        if is_test && let Some(exe) = artifact.executable {
            binaries.push(PathBuf::from(exe));
        }
    }
    binaries
}

fn run_tests(project_dir: &Path, crappy_dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
    if !rustflags.is_empty() {
        rustflags.push(' ');
    }
    rustflags.push_str("-Cinstrument-coverage");

    let proffile = crappy_dir.join("%m_%p.profraw");

    let output = Command::new("cargo")
        .args(["test", "--tests", "--message-format=json"])
        .env("CARGO_INCREMENTAL", "0")
        .env("RUSTFLAGS", &rustflags)
        .env("LLVM_PROFILE_FILE", &proffile)
        .current_dir(project_dir)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("{stderr}");
        return Err(Error::Command {
            tool: "cargo test",
            status: output.status,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let binaries = find_test_binaries(&stdout);

    if binaries.is_empty() {
        return Err(Error::NoTestBinaries);
    }

    Ok(binaries)
}

pub(crate) fn collect_profraw(crappy_dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut profraw_files = Vec::new();
    for entry in fs::read_dir(crappy_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "profraw") {
            profraw_files.push(path);
        }
    }
    Ok(profraw_files)
}

pub(crate) fn merge_profdata(tools: &LlvmTools, crappy_dir: &Path) -> Result<PathBuf, Error> {
    let profraw_files = collect_profraw(crappy_dir)?;

    if profraw_files.is_empty() {
        return Err(Error::NoProfrawFiles);
    }

    let profdata_path = crappy_dir.join("coverage.profdata");

    let mut cmd = Command::new(&tools.profdata);
    cmd.args(["merge", "-sparse", "-output"]);
    cmd.arg(&profdata_path);
    for f in &profraw_files {
        cmd.arg(f);
    }

    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::Command {
            tool: "llvm-profdata",
            status,
        });
    }

    Ok(profdata_path)
}

pub(crate) fn export_coverage(
    tools: &LlvmTools,
    profdata_path: &Path,
    binaries: &[PathBuf],
) -> Result<Vec<u8>, Error> {
    let mut cmd = Command::new(&tools.cov);
    cmd.args(["export", "-format=text"]);
    cmd.arg(format!("-instr-profile={}", profdata_path.display()));
    for bin in binaries {
        cmd.arg(format!("-object={}", bin.display()));
    }

    let output = cmd.output()?;
    if !output.status.success() {
        return Err(Error::Command {
            tool: "llvm-cov",
            status: output.status,
        });
    }

    Ok(output.stdout)
}

pub(crate) fn extract_function_coverage(
    llvm_cov_json: &[u8],
    project_prefix: &Path,
) -> Result<Vec<FunctionCoverage>, Error> {
    let export: LlvmCovExport = bourne::parse(llvm_cov_json)?;
    let mut results = Vec::new();

    for data in &export.data {
        for func in &data.functions {
            if let Some(fc) = convert_function(func, project_prefix) {
                results.push(fc);
            }
        }
    }

    Ok(results)
}

fn convert_function(func: &LlvmCovFunction, project_prefix: &Path) -> Option<FunctionCoverage> {
    let filename = func.filenames.first()?;
    let file_path = PathBuf::from(filename);

    if !file_path.starts_with(project_prefix) {
        return None;
    }

    if func.regions.is_empty() {
        return None;
    }

    let (start_line, end_line) = region_span(&func.regions)?;
    let line_coverage_pct = compute_line_coverage(&func.regions);

    Some(FunctionCoverage {
        file: file_path,
        start_line,
        end_line,
        line_coverage_pct,
    })
}

fn region_span(regions: &[Vec<u64>]) -> Option<(u32, u32)> {
    let mut start = u32::MAX;
    let mut end = 0u32;

    for region in regions {
        if region.len() >= 3 {
            start = start.min(region[0] as u32);
            end = end.max(region[2] as u32);
        }
    }

    if start == u32::MAX {
        None
    } else {
        Some((start, end))
    }
}

pub(crate) fn compute_line_coverage(regions: &[Vec<u64>]) -> f64 {
    let mut line_hits: HashMap<u32, u64> = HashMap::new();

    for region in regions {
        if region.len() < 5 {
            continue;
        }
        let start_line = region[0] as u32;
        let end_line = region[2] as u32;
        let count = region[4];

        // Kind is at index 7 if present; 0 = CodeRegion, skip others
        if region.len() > 7 && region[7] != 0 {
            continue;
        }

        for line in start_line..=end_line {
            let entry = line_hits.entry(line).or_insert(0);
            *entry = (*entry).max(count);
        }
    }

    if line_hits.is_empty() {
        return 100.0;
    }

    let total = line_hits.len() as f64;
    let covered = line_hits.values().filter(|&&c| c > 0).count() as f64;
    (covered / total) * 100.0
}

pub fn collect_coverage(project_dir: &Path) -> Result<Vec<FunctionCoverage>, Error> {
    let crappy_dir = project_dir.join("target").join("crappy");

    clean_profraw(&crappy_dir)?;
    let binaries = run_tests(project_dir, &crappy_dir)?;
    let tools = find_llvm_tools()?;
    let profdata_path = merge_profdata(&tools, &crappy_dir)?;
    let json = export_coverage(&tools, &profdata_path, &binaries)?;
    let project_prefix = project_dir.canonicalize()?;

    extract_function_coverage(&json, &project_prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(start: u64, end: u64, count: u64) -> Vec<u64> {
        vec![start, 1, end, 1, count, 0, 0, 0]
    }

    fn tmpdir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("crappy-cov-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mock_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    // --- compute_line_coverage ---

    #[test]
    fn fully_covered() {
        let regions = vec![region(1, 5, 3)];
        assert!((compute_line_coverage(&regions) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fully_uncovered() {
        let regions = vec![region(1, 5, 0)];
        assert!(compute_line_coverage(&regions).abs() < f64::EPSILON);
    }

    #[test]
    fn partial_coverage() {
        let regions = vec![region(1, 2, 1), region(3, 4, 0)];
        assert!((compute_line_coverage(&regions) - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_regions_returns_100() {
        assert!((compute_line_coverage(&[]) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn non_code_region_skipped() {
        let regions = vec![vec![1, 1, 5, 1, 0, 0, 0, 2]];
        assert!((compute_line_coverage(&regions) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn overlapping_regions_take_max_count() {
        let regions = vec![region(1, 3, 0), region(2, 3, 5)];
        let cov = compute_line_coverage(&regions);
        let expected = 2.0 / 3.0 * 100.0;
        assert!((cov - expected).abs() < 0.01);
    }

    #[test]
    fn short_region_ignored() {
        let regions = vec![vec![1, 2, 3]];
        assert!((compute_line_coverage(&regions) - 100.0).abs() < f64::EPSILON);
    }

    // --- region_span ---

    #[test]
    fn region_span_basic() {
        let regions = vec![region(5, 10, 1), region(12, 20, 0)];
        assert_eq!(region_span(&regions), Some((5, 20)));
    }

    #[test]
    fn region_span_empty() {
        assert_eq!(region_span(&[]), None);
    }

    // --- find_test_binaries ---

    #[test]
    fn find_test_binaries_parses_artifacts() {
        let stdout = r#"{"reason":"compiler-artifact","package_id":"test","executable":"/bin/test1","profile":{"test":true},"target":{"kind":["lib"]},"features":[],"filenames":[],"fresh":false}
{"reason":"compiler-artifact","package_id":"test","executable":null,"profile":{"test":false},"target":{"kind":["lib"]},"features":[],"filenames":[],"fresh":false}
not json at all
{"reason":"build-finished","success":true}
"#;
        let bins = find_test_binaries(stdout);
        assert_eq!(bins, vec![PathBuf::from("/bin/test1")]);
    }

    #[test]
    fn find_test_binaries_skips_non_test_profile() {
        let stdout = r#"{"reason":"compiler-artifact","package_id":"x","executable":"/bin/x","profile":{"test":false},"target":{"kind":["lib"]},"features":[],"filenames":[],"fresh":false}"#;
        assert!(find_test_binaries(stdout).is_empty());
    }

    #[test]
    fn find_test_binaries_skips_null_executable() {
        let stdout = r#"{"reason":"compiler-artifact","package_id":"x","executable":null,"profile":{"test":true},"target":{"kind":["lib"]},"features":[],"filenames":[],"fresh":false}"#;
        assert!(find_test_binaries(stdout).is_empty());
    }

    // --- extract_function_coverage ---

    #[test]
    fn extract_function_coverage_filters_by_prefix() {
        let json = br#"{"data":[{"functions":[
            {"name":"f","filenames":["/proj/src/lib.rs"],"regions":[[1,1,5,1,3,0,0,0]],"count":3},
            {"name":"g","filenames":["/other/src/lib.rs"],"regions":[[1,1,5,1,1,0,0,0]],"count":1}
        ]}]}"#;
        let results = extract_function_coverage(json, Path::new("/proj")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file, PathBuf::from("/proj/src/lib.rs"));
    }

    #[test]
    fn extract_skips_empty_regions() {
        let json = br#"{"data":[{"functions":[
            {"name":"f","filenames":["/proj/src/lib.rs"],"regions":[],"count":0}
        ]}]}"#;
        let results = extract_function_coverage(json, Path::new("/proj")).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn extract_skips_no_filenames() {
        let json = br#"{"data":[{"functions":[
            {"name":"f","filenames":[],"regions":[[1,1,5,1,1,0,0,0]],"count":1}
        ]}]}"#;
        let results = extract_function_coverage(json, Path::new("/proj")).unwrap();
        assert!(results.is_empty());
    }

    // --- clean_profraw ---

    #[test]
    fn clean_profraw_removes_profraw_files() {
        let dir = tmpdir("rm");
        fs::write(dir.join("a.profraw"), "").unwrap();
        fs::write(dir.join("b.profraw"), "").unwrap();
        fs::write(dir.join("keep.profdata"), "").unwrap();

        clean_profraw(&dir).unwrap();

        let remaining: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].file_name(), "keep.profdata");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_profraw_creates_missing_dir() {
        let dir = tmpdir("mkdir").join("nonexistent");
        assert!(!dir.exists());

        clean_profraw(&dir).unwrap();

        assert!(dir.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_profraw_empty_dir_is_ok() {
        let dir = tmpdir("empty");
        clean_profraw(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    // --- parse_rustc_host ---

    #[test]
    fn parse_host_from_version_output() {
        let output = "rustc 1.89.0 (abc123 2025-01-01)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.89.0\n";
        assert_eq!(
            parse_rustc_host(output),
            Some("x86_64-unknown-linux-gnu".into())
        );
    }

    #[test]
    fn parse_host_missing() {
        assert_eq!(parse_rustc_host("no host here"), None);
    }

    // --- resolve_llvm_tools ---

    #[test]
    fn resolve_finds_tools_in_sysroot() {
        let dir = tmpdir("sysroot");
        let bin = dir
            .join("lib")
            .join("rustlib")
            .join("x86_64-unknown-linux-gnu")
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        mock_script(&bin, "llvm-profdata", "true");
        mock_script(&bin, "llvm-cov", "true");

        let tools = resolve_llvm_tools(dir.to_str().unwrap(), "x86_64-unknown-linux-gnu").unwrap();
        assert!(tools.profdata.exists());
        assert!(tools.cov.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_errors_on_missing_profdata() {
        let dir = tmpdir("noprof");
        let bin = dir
            .join("lib")
            .join("rustlib")
            .join("x86_64-unknown-linux-gnu")
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        mock_script(&bin, "llvm-cov", "true");

        let err = resolve_llvm_tools(dir.to_str().unwrap(), "x86_64-unknown-linux-gnu");
        assert!(err.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_errors_on_missing_cov() {
        let dir = tmpdir("nocov");
        let bin = dir
            .join("lib")
            .join("rustlib")
            .join("x86_64-unknown-linux-gnu")
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        mock_script(&bin, "llvm-profdata", "true");

        let err = resolve_llvm_tools(dir.to_str().unwrap(), "x86_64-unknown-linux-gnu");
        assert!(err.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    // --- collect_profraw ---

    #[test]
    fn collect_profraw_finds_files() {
        let dir = tmpdir("profraw");
        fs::write(dir.join("a.profraw"), "").unwrap();
        fs::write(dir.join("b.profraw"), "").unwrap();
        fs::write(dir.join("c.txt"), "").unwrap();

        let files = collect_profraw(&dir).unwrap();
        assert_eq!(files.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- merge_profdata with mock ---

    #[test]
    fn merge_profdata_calls_tool_and_returns_path() {
        let dir = tmpdir("merge");
        fs::write(dir.join("a.profraw"), "").unwrap();

        let profdata_script = mock_script(&dir, "mock-profdata", "touch \"$4\"");
        let tools = LlvmTools {
            profdata: profdata_script,
            cov: PathBuf::from("/dev/null"),
        };

        let result = merge_profdata(&tools, &dir).unwrap();
        assert_eq!(result, dir.join("coverage.profdata"));
        assert!(result.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_profdata_errors_on_no_profraw() {
        let dir = tmpdir("nomerge");
        let tools = LlvmTools {
            profdata: PathBuf::from("/dev/null"),
            cov: PathBuf::from("/dev/null"),
        };

        let err = merge_profdata(&tools, &dir);
        assert!(matches!(err, Err(Error::NoProfrawFiles)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_profdata_errors_on_tool_failure() {
        let dir = tmpdir("mergefail");
        fs::write(dir.join("a.profraw"), "").unwrap();

        let profdata_script = mock_script(&dir, "mock-profdata-fail", "exit 1");
        let tools = LlvmTools {
            profdata: profdata_script,
            cov: PathBuf::from("/dev/null"),
        };

        let err = merge_profdata(&tools, &dir);
        assert!(err.is_err(), "expected error, got: {err:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    // --- export_coverage with mock ---

    #[test]
    fn export_coverage_returns_stdout() {
        let dir = tmpdir("export");
        let json = r#"{"data":[]}"#;
        let cov_script = mock_script(&dir, "mock-cov", &format!("echo '{json}'"));
        let tools = LlvmTools {
            profdata: PathBuf::from("/dev/null"),
            cov: cov_script,
        };

        let result = export_coverage(
            &tools,
            Path::new("/fake.profdata"),
            &[PathBuf::from("/fake/bin")],
        )
        .unwrap();
        let output = String::from_utf8(result).unwrap();
        assert!(output.contains(r#""data":[]"#));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_coverage_errors_on_tool_failure() {
        let dir = tmpdir("exportfail");
        let cov_script = mock_script(&dir, "mock-cov-fail", "exit 1");
        let tools = LlvmTools {
            profdata: PathBuf::from("/dev/null"),
            cov: cov_script,
        };

        let err = export_coverage(&tools, Path::new("/fake.profdata"), &[]);
        assert!(err.is_err(), "expected error, got: {err:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
