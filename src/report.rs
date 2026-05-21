use std::io::Write;

use crate::Opts;
use crate::color;
use crate::scoring::CrapRecord;

pub fn print_report(records: &[CrapRecord], opts: &Opts) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    write_report(&mut out, records, opts).unwrap();
}

pub fn write_report(
    out: &mut impl Write,
    records: &[CrapRecord],
    opts: &Opts,
) -> std::io::Result<()> {
    let display: &[CrapRecord] = opts
        .top
        .map_or(records, |n| &records[..n.min(records.len())]);

    if display.is_empty() {
        writeln!(out, "No functions to report.")?;
        return Ok(());
    }

    let show_all = opts.top.is_some();
    let min_score = opts.threshold.unwrap_or(30.0);
    for r in display {
        if !show_all && r.crappy_score < min_score {
            continue;
        }
        if is_clean(r) {
            continue;
        }
        write_diagnostic(out, r, min_score)?;
    }

    write_summary(out, records, opts)
}

fn is_clean(r: &CrapRecord) -> bool {
    r.coverage_pct >= 50.0
        && r.complexity < 10
        && r.idiom_penalty <= 1.0
        && r.checks.is_empty()
        && !r.sig_duplicate
        && !r.body_duplicate
}

fn lint_code(r: &CrapRecord) -> &'static str {
    if r.coverage_pct < 50.0 && r.complexity >= 10 {
        "high-risk"
    } else if r.coverage_pct < 50.0 {
        "low-coverage"
    } else if r.complexity >= 10 {
        "high-complexity"
    } else if r.idiom_penalty > 1.0 {
        "idiom-violation"
    } else {
        "risky"
    }
}

fn colored_level(score: f64, threshold: f64) -> String {
    if score >= threshold {
        color::bold_yellow("warning")
    } else {
        color::bold_cyan("note")
    }
}

fn write_diagnostic(out: &mut impl Write, r: &CrapRecord, threshold: f64) -> std::io::Result<()> {
    let level = colored_level(r.crappy_score, threshold);
    let code = lint_code(r);
    writeln!(
        out,
        "{}[{}]: `{}` has high change-risk score",
        level,
        color::bold_white(code),
        color::bold_white(&r.name),
    )?;
    writeln!(
        out,
        " {} {}:{}",
        color::bold_blue("-->"),
        r.file,
        r.start_line
    )?;
    writeln!(out, "  {}", color::bold_blue("|"))?;
    writeln!(
        out,
        "  {} {} CRAPPY = {:.1}, CRAP = {:.1}, CC = {}, Cov = {:.0}%",
        color::bold_blue("="),
        color::bold_white("note:"),
        r.crappy_score,
        r.crap_score,
        r.complexity,
        r.coverage_pct,
    )?;

    if r.coverage_pct < 50.0 {
        writeln!(
            out,
            "  {} {} coverage is {:.0}% — add tests to reduce risk",
            color::bold_blue("="),
            color::bold_white("help:"),
            r.coverage_pct
        )?;
    }
    if r.complexity >= 10 {
        writeln!(
            out,
            "  {} {} cyclomatic complexity is {} — consider splitting into smaller functions",
            color::bold_blue("="),
            color::bold_white("help:"),
            r.complexity
        )?;
    }
    if r.idiom_penalty > 1.0 {
        writeln!(
            out,
            "  {} {} idiom penalty {:.2}x applied",
            color::bold_blue("="),
            color::bold_white("note:"),
            r.idiom_penalty
        )?;
    }
    for check in &r.checks {
        writeln!(
            out,
            "  {} {} {}",
            color::bold_blue("="),
            color::bold_white("help:"),
            check.suggestion()
        )?;
    }
    if r.sig_duplicate {
        writeln!(
            out,
            "  {} {} signature duplicate — another function has the same (params)->return type",
            color::bold_blue("="),
            color::bold_white("help:"),
        )?;
    }
    if r.body_duplicate {
        writeln!(
            out,
            "  {} {} body duplicate — another function has the same structure",
            color::bold_blue("="),
            color::bold_white("help:"),
        )?;
    }

    writeln!(out)?;
    Ok(())
}

fn write_summary(out: &mut impl Write, records: &[CrapRecord], opts: &Opts) -> std::io::Result<()> {
    let threshold = opts.threshold.unwrap_or(30.0);
    let warnings = records
        .iter()
        .filter(|r| r.crappy_score >= threshold)
        .count();

    if warnings > 0 {
        if opts.threshold.is_some() {
            writeln!(
                out,
                "{}: `cargo crappy` generated {warnings} warnings (threshold {threshold:.0})",
                color::bold_yellow("warning"),
            )
        } else {
            writeln!(
                out,
                "{}: `cargo crappy` generated {warnings} warnings",
                color::bold_yellow("warning"),
            )
        }
    } else {
        writeln!(
            out,
            "{}: `cargo crappy` analyzed {} functions, no warnings",
            color::bold_cyan("note"),
            records.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idiom::IdiomCheck;
    use crate::scoring::CrapRecord;

    fn record(name: &str, cc: u32, cov: f64, score: f64) -> CrapRecord {
        CrapRecord {
            file: "src/lib.rs".into(),
            name: name.into(),
            complexity: cc,
            coverage_pct: cov,
            crap_score: score,
            idiom_penalty: 1.0,
            crappy_score: score,
            start_line: 1,
            checks: Vec::new(),
            sig_duplicate: false,
            body_duplicate: false,
        }
    }

    fn record_with_violations(
        name: &str,
        score: f64,
        penalty: f64,
        checks: Vec<IdiomCheck>,
    ) -> CrapRecord {
        CrapRecord {
            file: "src/lib.rs".into(),
            name: name.into(),
            complexity: 5,
            coverage_pct: 50.0,
            crap_score: score,
            idiom_penalty: penalty,
            crappy_score: score * penalty,
            start_line: 10,
            checks,
            sig_duplicate: false,
            body_duplicate: false,
        }
    }

    #[test]
    fn empty_report() {
        let mut buf = Vec::new();
        let opts = Opts::default();
        write_report(&mut buf, &[], &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("No functions to report."));
    }

    #[test]
    fn diagnostic_has_location_and_lint_code() {
        let mut buf = Vec::new();
        let records = vec![record("add", 5, 0.0, 30.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("`add`"), "should contain function name: {out}");
        assert!(
            out.contains("--> src/lib.rs:1"),
            "should contain location: {out}"
        );
        assert!(
            out.contains("[low-coverage]"),
            "should have lint code: {out}"
        );
        assert!(out.contains("  |"), "should have gutter: {out}");
    }

    #[test]
    fn warning_level_for_high_score() {
        let mut buf = Vec::new();
        let records = vec![record("bad", 10, 0.0, 110.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.starts_with("warning["), "high score = warning: {out}");
    }

    #[test]
    fn note_level_for_low_score() {
        let mut buf = Vec::new();
        let records = vec![record("borderline", 3, 20.0, 15.0)];
        let opts = Opts {
            top: Some(10),
            ..Opts::default()
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.starts_with("note["), "low score = note: {out}");
    }

    #[test]
    fn lint_code_low_coverage() {
        let mut buf = Vec::new();
        let records = vec![record("untested", 3, 10.0, 30.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("[low-coverage]"),
            "low cov = low-coverage: {out}"
        );
    }

    #[test]
    fn lint_code_high_complexity() {
        let mut buf = Vec::new();
        let records = vec![record("complex", 15, 80.0, 15.0)];
        let opts = Opts {
            top: Some(10),
            ..Opts::default()
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("[high-complexity]"),
            "high cc = high-complexity: {out}"
        );
    }

    #[test]
    fn lint_code_high_risk() {
        let mut buf = Vec::new();
        let records = vec![record("danger", 12, 10.0, 110.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("[high-risk]"),
            "low cov + high cc = high-risk: {out}"
        );
    }

    #[test]
    fn metrics_in_note_line() {
        let mut buf = Vec::new();
        let records = vec![record("f", 5, 10.0, 30.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("= note: CRAPPY"),
            "metrics on note line: {out}"
        );
    }

    #[test]
    fn low_coverage_suggestion() {
        let mut buf = Vec::new();
        let records = vec![record("untested", 5, 10.0, 30.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("add tests"), "should suggest tests: {out}");
    }

    #[test]
    fn high_complexity_suggestion() {
        let mut buf = Vec::new();
        let records = vec![record("complex", 15, 80.0, 15.0)];
        let opts = Opts {
            top: Some(10),
            ..Opts::default()
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("splitting"), "should suggest splitting: {out}");
    }

    #[test]
    fn idiom_violations_listed() {
        let mut buf = Vec::new();
        let records = vec![record_with_violations(
            "bad",
            30.0,
            1.5,
            vec![IdiomCheck::Unwrap, IdiomCheck::EmptyVecMacro],
        )];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains(".unwrap()"), "should list unwrap: {out}");
        assert!(out.contains("Vec::new()"), "should list vec![]: {out}");
        assert!(
            out.contains("= note: idiom penalty 1.50x"),
            "should show penalty: {out}"
        );
    }

    #[test]
    fn dry_violations_listed() {
        let mut buf = Vec::new();
        let mut r = record("dup", 5, 0.0, 30.0);
        r.sig_duplicate = true;
        r.body_duplicate = true;
        let records = vec![r];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("signature duplicate"),
            "should list sig dup: {out}"
        );
        assert!(
            out.contains("body duplicate"),
            "should list body dup: {out}"
        );
    }

    #[test]
    fn top_limits_output() {
        let mut buf = Vec::new();
        let records = vec![
            record("a", 5, 0.0, 30.0),
            record("b", 3, 20.0, 10.0),
            record("c", 2, 10.0, 5.0),
        ];
        let opts = Opts {
            top: Some(2),
            ..Opts::default()
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("`a`"), "should show a: {out}");
        assert!(out.contains("`b`"), "should show b: {out}");
        assert!(!out.contains("`c`"), "should not show c: {out}");
    }

    #[test]
    fn threshold_in_summary() {
        let mut buf = Vec::new();
        let records = vec![record("bad", 10, 0.0, 110.0), record("ok", 1, 100.0, 1.0)];
        let opts = Opts {
            threshold: Some(30.0),
            ..Opts::default()
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("1 warnings (threshold 30)"),
            "should show threshold: {out}"
        );
    }

    #[test]
    fn no_threshold_omits_count() {
        let mut buf = Vec::new();
        let records = vec![record("f", 1, 100.0, 1.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains("threshold"), "no threshold: {out}");
    }

    #[test]
    fn clean_function_no_help_lines() {
        let mut buf = Vec::new();
        let records = vec![record("clean", 1, 100.0, 1.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains("= help:"), "clean fn has no help: {out}");
    }

    #[test]
    fn summary_warning_count() {
        let mut buf = Vec::new();
        let records = vec![
            record("bad1", 10, 0.0, 110.0),
            record("bad2", 8, 0.0, 72.0),
            record("ok", 1, 100.0, 1.0),
        ];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("generated 2 warnings"),
            "should count warnings: {out}"
        );
    }

    #[test]
    fn no_warnings_summary() {
        let mut buf = Vec::new();
        let records = vec![record("ok", 1, 100.0, 1.0)];
        let opts = Opts::default();
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("no warnings"),
            "clean run = no warnings: {out}"
        );
    }
}
