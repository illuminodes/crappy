use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::complexity::FunctionComplexity;
use crate::coverage::FunctionCoverage;
use crate::idiom::FunctionIdioms;

pub struct CrapRecord {
    pub file: String,
    pub name: String,
    pub complexity: u32,
    pub coverage_pct: f64,
    pub crap_score: f64,
    pub idiom_penalty: f64,
    pub crappy_score: f64,
    pub start_line: u32,
}

pub(crate) fn crap_score(complexity: u32, coverage_pct: f64) -> f64 {
    let comp = f64::from(complexity);
    let cov = coverage_pct / 100.0;
    comp * comp * (1.0 - cov).powi(3) + comp
}

pub(crate) fn idiom_penalty(demerits: u32, body_lines: u32) -> f64 {
    1.0 + (f64::from(demerits) / f64::from(body_lines.max(1)))
}

pub(crate) fn overlap(a_start: u32, a_end: u32, b_start: u32, b_end: u32) -> u32 {
    if a_start > b_end || b_start > a_end {
        return 0;
    }
    a_end.min(b_end) - a_start.max(b_start) + 1
}

pub fn compute_crap_scores(
    coverage: Vec<FunctionCoverage>,
    complexity: Vec<FunctionComplexity>,
    idioms: Vec<FunctionIdioms>,
    project_dir: &Path,
) -> Vec<CrapRecord> {
    let project_prefix = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());

    let mut cov_by_file: HashMap<PathBuf, Vec<FunctionCoverage>> = HashMap::new();
    for fc in coverage {
        cov_by_file.entry(fc.file.clone()).or_default().push(fc);
    }

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

    let mut idiom_map: HashMap<(PathBuf, String), FunctionIdioms> = HashMap::new();
    for fi in idioms {
        idiom_map.insert((fi.file.clone(), fi.qualified_name.clone()), fi);
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

        let idiom_key = (func.file.clone(), func.qualified_name.clone());
        let (demerits, body_lines) = idiom_map
            .get(&idiom_key)
            .map(|fi| {
                let lines = fi.end_line.saturating_sub(fi.start_line) + 1;
                (fi.demerits, lines)
            })
            .unwrap_or((0, 1));

        let penalty = idiom_penalty(demerits, body_lines);
        let crappy = score * penalty;

        records.push(CrapRecord {
            file: file_display,
            name: func.qualified_name.clone(),
            complexity: func.complexity,
            coverage_pct,
            crap_score: score,
            idiom_penalty: penalty,
            crappy_score: crappy,
            start_line: func.start_line,
        });
    }

    records.sort_by(|a, b| b.crappy_score.partial_cmp(&a.crappy_score).unwrap());
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trivial_fully_covered() {
        assert!((crap_score(1, 100.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn trivial_uncovered() {
        assert!((crap_score(1, 0.0) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn published_example() {
        assert!((crap_score(6, 0.0) - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cc12_uncovered() {
        assert!((crap_score(12, 0.0) - 156.0).abs() < f64::EPSILON);
    }

    #[test]
    fn full_coverage_equals_complexity() {
        for cc in 1..=50 {
            let score = crap_score(cc, 100.0);
            assert!(
                (score - f64::from(cc)).abs() < f64::EPSILON,
                "cc={cc} score={score}"
            );
        }
    }

    #[test]
    fn score_monotonically_decreases_with_coverage() {
        let mut prev = crap_score(10, 0.0);
        for cov in (10..=100).step_by(10) {
            let cur = crap_score(10, cov as f64);
            assert!(cur <= prev, "cov={cov} cur={cur} prev={prev}");
            prev = cur;
        }
    }

    #[test]
    fn idiom_penalty_zero_demerits() {
        assert!((idiom_penalty(0, 10) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn idiom_penalty_proportional() {
        // 2 demerits in 10 lines → 1.2
        assert!((idiom_penalty(2, 10) - 1.2).abs() < f64::EPSILON);
    }

    #[test]
    fn idiom_penalty_dense() {
        // 5 demerits in 5 lines → 2.0
        assert!((idiom_penalty(5, 5) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn idiom_penalty_zero_lines_safe() {
        assert!((idiom_penalty(1, 0) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn overlap_full() {
        assert_eq!(overlap(1, 10, 1, 10), 10);
    }

    #[test]
    fn overlap_partial() {
        assert_eq!(overlap(1, 5, 3, 8), 3);
    }

    #[test]
    fn overlap_none() {
        assert_eq!(overlap(1, 5, 6, 10), 0);
    }

    #[test]
    fn overlap_adjacent() {
        assert_eq!(overlap(1, 5, 5, 10), 1);
    }

    #[test]
    fn overlap_subset() {
        assert_eq!(overlap(3, 7, 1, 10), 5);
    }

    #[test]
    fn join_matches_by_line_overlap() {
        let cov = vec![FunctionCoverage {
            file: PathBuf::from("/proj/src/lib.rs"),
            start_line: 5,
            end_line: 15,
            line_coverage_pct: 80.0,
        }];
        let comp = vec![FunctionComplexity {
            file: PathBuf::from("/proj/src/lib.rs"),
            qualified_name: "my_func".into(),
            start_line: 4,
            end_line: 16,
            complexity: 5,
        }];

        let records = compute_crap_scores(cov, comp, vec![], Path::new("/proj"));
        assert_eq!(records.len(), 1);
        assert!((records[0].coverage_pct - 80.0).abs() < f64::EPSILON);
        assert_eq!(records[0].complexity, 5);
    }

    #[test]
    fn unmatched_complexity_gets_zero_coverage() {
        let cov = vec![];
        let comp = vec![FunctionComplexity {
            file: PathBuf::from("/proj/src/lib.rs"),
            qualified_name: "orphan".into(),
            start_line: 1,
            end_line: 5,
            complexity: 3,
        }];

        let records = compute_crap_scores(cov, comp, vec![], Path::new("/proj"));
        assert_eq!(records.len(), 1);
        assert!((records[0].coverage_pct).abs() < f64::EPSILON);
        assert!((records[0].crap_score - 12.0).abs() < f64::EPSILON);
    }

    #[test]
    fn results_sorted_by_crappy_score_descending() {
        let cov = vec![];
        let comp = vec![
            FunctionComplexity {
                file: PathBuf::from("/proj/src/a.rs"),
                qualified_name: "low".into(),
                start_line: 1,
                end_line: 2,
                complexity: 1,
            },
            FunctionComplexity {
                file: PathBuf::from("/proj/src/a.rs"),
                qualified_name: "high".into(),
                start_line: 5,
                end_line: 20,
                complexity: 10,
            },
        ];

        let records = compute_crap_scores(cov, comp, vec![], Path::new("/proj"));
        assert_eq!(records[0].name, "high");
        assert_eq!(records[1].name, "low");
    }

    #[test]
    fn idiom_demerits_multiply_crap() {
        let comp = vec![FunctionComplexity {
            file: PathBuf::from("/proj/src/lib.rs"),
            qualified_name: "f".into(),
            start_line: 1,
            end_line: 10,
            complexity: 5,
        }];
        let idioms = vec![FunctionIdioms {
            file: PathBuf::from("/proj/src/lib.rs"),
            qualified_name: "f".into(),
            start_line: 1,
            end_line: 10,
            demerits: 5,
        }];

        let records = compute_crap_scores(vec![], comp, idioms, Path::new("/proj"));
        // CRAP = 5^2 * 1 + 5 = 30, penalty = 1 + 5/10 = 1.5, CRAPPY = 45
        assert!((records[0].crap_score - 30.0).abs() < f64::EPSILON);
        assert!((records[0].crappy_score - 45.0).abs() < f64::EPSILON);
    }

    #[test]
    fn clean_idioms_no_penalty() {
        let comp = vec![FunctionComplexity {
            file: PathBuf::from("/proj/src/lib.rs"),
            qualified_name: "f".into(),
            start_line: 1,
            end_line: 10,
            complexity: 5,
        }];

        let records = compute_crap_scores(vec![], comp, vec![], Path::new("/proj"));
        assert!((records[0].crap_score - records[0].crappy_score).abs() < f64::EPSILON);
    }
}
