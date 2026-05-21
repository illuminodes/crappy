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

struct LlvmTools {
    profdata: PathBuf,
    cov: PathBuf,
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
    let host = version_str
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .ok_or_else(|| Error::ToolNotFound {
            tool: "rustc host triple".into(),
        })?
        .to_string();

    let bin_dir = PathBuf::from(&sysroot)
        .join("lib")
        .join("rustlib")
        .join(&host)
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

fn merge_profdata(tools: &LlvmTools, crappy_dir: &Path) -> Result<PathBuf, Error> {
    let mut profraw_files = Vec::new();
    for entry in fs::read_dir(crappy_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "profraw") {
            profraw_files.push(path);
        }
    }

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

fn export_coverage(
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

    #[test]
    fn region_span_basic() {
        let regions = vec![region(5, 10, 1), region(12, 20, 0)];
        assert_eq!(region_span(&regions), Some((5, 20)));
    }

    #[test]
    fn region_span_empty() {
        assert_eq!(region_span(&[]), None);
    }

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
}
