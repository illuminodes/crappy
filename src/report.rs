use std::io::Write;

use crate::Opts;
use crate::scoring::CrapRecord;

pub fn print_report(records: &[CrapRecord], opts: &Opts) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    write_report(&mut out, records, opts).unwrap();
}

pub(crate) fn write_report(
    out: &mut impl Write,
    records: &[CrapRecord],
    opts: &Opts,
) -> std::io::Result<()> {
    let display: &[CrapRecord] = if let Some(n) = opts.top {
        &records[..n.min(records.len())]
    } else {
        records
    };

    if display.is_empty() {
        writeln!(out, "No functions to report.")?;
        return Ok(());
    }

    let name_width = display
        .iter()
        .map(|r| format!("{}:{} {}", r.file, r.start_line, r.name).len())
        .max()
        .unwrap_or(20)
        .max(8);

    writeln!(
        out,
        "{:>6} | {:>4} | {:>6} | {:<width$}",
        "CRAP",
        "CC",
        "Cov%",
        "Function",
        width = name_width,
    )?;
    writeln!(
        out,
        "{:-<6}-+-{:-<4}-+-{:-<6}-+-{:-<width$}",
        "",
        "",
        "",
        "",
        width = name_width,
    )?;

    for r in display {
        let location = format!("{}:{}", r.file, r.start_line);
        writeln!(
            out,
            "{:>6.1} | {:>4} | {:>5.1}% | {location} {}",
            r.crap_score, r.complexity, r.coverage_pct, r.name,
        )?;
    }

    writeln!(out)?;
    writeln!(out, "Total functions: {}", records.len())?;

    if let Some(threshold) = opts.threshold {
        let above = records.iter().filter(|r| r.crap_score > threshold).count();
        writeln!(
            out,
            "Functions above CRAP threshold ({threshold:.0}): {above}"
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, cc: u32, cov: f64, score: f64) -> CrapRecord {
        CrapRecord {
            file: "src/lib.rs".into(),
            name: name.into(),
            complexity: cc,
            coverage_pct: cov,
            crap_score: score,
            start_line: 1,
        }
    }

    #[test]
    fn empty_report() {
        let mut buf = Vec::new();
        let opts = Opts {
            threshold: None,
            top: None,
        };
        write_report(&mut buf, &[], &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("No functions to report."));
    }

    #[test]
    fn basic_report_has_header_and_row() {
        let mut buf = Vec::new();
        let records = vec![record("add", 1, 100.0, 1.0)];
        let opts = Opts {
            threshold: None,
            top: None,
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("CRAP"));
        assert!(out.contains("CC"));
        assert!(out.contains("Cov%"));
        assert!(out.contains("add"));
        assert!(out.contains("Total functions: 1"));
    }

    #[test]
    fn top_limits_output() {
        let mut buf = Vec::new();
        let records = vec![
            record("a", 5, 0.0, 30.0),
            record("b", 3, 50.0, 10.0),
            record("c", 1, 100.0, 1.0),
        ];
        let opts = Opts {
            threshold: None,
            top: Some(2),
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("a"));
        assert!(out.contains("b"));
        assert!(!out.contains(" c"));
        assert!(out.contains("Total functions: 3"));
    }

    #[test]
    fn threshold_shows_count() {
        let mut buf = Vec::new();
        let records = vec![record("bad", 10, 0.0, 110.0), record("ok", 1, 100.0, 1.0)];
        let opts = Opts {
            threshold: Some(30.0),
            top: None,
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Functions above CRAP threshold (30): 1"));
    }

    #[test]
    fn no_threshold_omits_line() {
        let mut buf = Vec::new();
        let records = vec![record("f", 1, 100.0, 1.0)];
        let opts = Opts {
            threshold: None,
            top: None,
        };
        write_report(&mut buf, &records, &opts).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(!out.contains("threshold"));
    }
}
