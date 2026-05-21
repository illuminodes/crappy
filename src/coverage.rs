use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::Error;

pub struct FunctionCoverage {
    pub file: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub line_coverage_pct: f64,
}

#[derive(Deserialize)]
struct LlvmCovExport {
    data: Vec<LlvmCovData>,
}

#[derive(Deserialize)]
struct LlvmCovData {
    functions: Vec<LlvmCovFunction>,
}

#[derive(Deserialize)]
struct LlvmCovFunction {
    filenames: Vec<String>,
    regions: Vec<Vec<serde_json::Value>>,
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
    let mut binaries = Vec::new();

    for line in stdout.lines() {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if val.get("reason").and_then(|v| v.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let Some(exe) = val.get("executable").and_then(|v| v.as_str()) else {
            continue;
        };
        let is_test = val
            .get("profile")
            .and_then(|p| p.get("test"))
            .and_then(|t| t.as_bool())
            .unwrap_or(false);
        if is_test {
            binaries.push(PathBuf::from(exe));
        }
    }

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
) -> Result<LlvmCovExport, Error> {
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

    let export: LlvmCovExport = serde_json::from_slice(&output.stdout)?;
    Ok(export)
}

fn compute_line_coverage(regions: &[Vec<serde_json::Value>]) -> f64 {
    let mut line_hits: HashMap<u32, u64> = HashMap::new();

    for region in regions {
        if region.len() < 5 {
            continue;
        }
        let Some(start_line) = region[0].as_u64() else {
            continue;
        };
        let Some(end_line) = region[2].as_u64() else {
            continue;
        };
        let Some(count) = region[4].as_u64() else {
            continue;
        };

        // Kind is at index 7 if present; 0 = CodeRegion, skip others (gap, expansion, skipped)
        if region.len() > 7
            && let Some(kind) = region[7].as_u64()
            && kind != 0
        {
            continue;
        }

        for line in (start_line as u32)..=(end_line as u32) {
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
    let export = export_coverage(&tools, &profdata_path, &binaries)?;

    let project_prefix = project_dir.canonicalize()?;

    let mut results = Vec::new();

    for data in &export.data {
        for func in &data.functions {
            let Some(filename) = func.filenames.first() else {
                continue;
            };
            let file_path = PathBuf::from(filename);

            if !file_path.starts_with(&project_prefix) {
                continue;
            }

            if func.regions.is_empty() {
                continue;
            }

            let mut start_line = u32::MAX;
            let mut end_line = 0u32;

            for region in &func.regions {
                if region.len() >= 3
                    && let (Some(sl), Some(el)) = (region[0].as_u64(), region[2].as_u64())
                {
                    start_line = start_line.min(sl as u32);
                    end_line = end_line.max(el as u32);
                }
            }

            if start_line == u32::MAX {
                continue;
            }

            let line_coverage_pct = compute_line_coverage(&func.regions);

            results.push(FunctionCoverage {
                file: file_path,
                start_line,
                end_line,
                line_coverage_pct,
            });
        }
    }

    Ok(results)
}
