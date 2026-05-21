use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::complexity::FunctionComplexity;
use crate::coverage::FunctionCoverage;

pub struct CrapRecord {
    pub file: String,
    pub name: String,
    pub complexity: u32,
    pub coverage_pct: f64,
    pub crap_score: f64,
    pub start_line: u32,
}

fn crap_score(complexity: u32, coverage_pct: f64) -> f64 {
    let comp = f64::from(complexity);
    let cov = coverage_pct / 100.0;
    comp * comp * (1.0 - cov).powi(3) + comp
}

fn overlap(a_start: u32, a_end: u32, b_start: u32, b_end: u32) -> u32 {
    if a_start > b_end || b_start > a_end {
        return 0;
    }
    a_end.min(b_end) - a_start.max(b_start) + 1
}

pub fn compute_crap_scores(
    coverage: Vec<FunctionCoverage>,
    complexity: Vec<FunctionComplexity>,
    project_dir: &Path,
) -> Vec<CrapRecord> {
    let project_prefix = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());

    let mut cov_by_file: HashMap<PathBuf, Vec<FunctionCoverage>> = HashMap::new();
    for fc in coverage {
        cov_by_file.entry(fc.file.clone()).or_default().push(fc);
    }

    // Merge monomorphized entries with same file+line range
    for entries in cov_by_file.values_mut() {
        entries.sort_by_key(|e| (e.start_line, e.end_line));

        let mut i = 0;
        while i + 1 < entries.len() {
            if entries[i].start_line == entries[i + 1].start_line
                && entries[i].end_line == entries[i + 1].end_line
            {
                let merged_cov = entries[i]
                    .line_coverage_pct
                    .max(entries[i + 1].line_coverage_pct);
                entries[i].line_coverage_pct = merged_cov;
                entries.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }

    let mut records = Vec::new();

    for func in &complexity {
        let rel_path = func
            .file
            .strip_prefix(&project_prefix)
            .unwrap_or(&func.file);
        let file_display = rel_path.display().to_string();

        let coverage_pct = cov_by_file
            .get(&func.file)
            .and_then(|entries| {
                entries
                    .iter()
                    .filter(|cov| {
                        overlap(func.start_line, func.end_line, cov.start_line, cov.end_line) > 0
                    })
                    .max_by_key(|cov| {
                        overlap(func.start_line, func.end_line, cov.start_line, cov.end_line)
                    })
                    .map(|cov| cov.line_coverage_pct)
            })
            .unwrap_or(0.0);

        let score = crap_score(func.complexity, coverage_pct);

        records.push(CrapRecord {
            file: file_display,
            name: func.qualified_name.clone(),
            complexity: func.complexity,
            coverage_pct,
            crap_score: score,
            start_line: func.start_line,
        });
    }

    records.sort_by(|a, b| b.crap_score.partial_cmp(&a.crap_score).unwrap());
    records
}
