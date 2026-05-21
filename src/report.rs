use crate::Opts;
use crate::scoring::CrapRecord;

pub fn print_report(records: &[CrapRecord], opts: &Opts) {
    let display: &[CrapRecord] = if let Some(n) = opts.top {
        &records[..n.min(records.len())]
    } else {
        records
    };

    if display.is_empty() {
        println!("No functions to report.");
        return;
    }

    let name_width = display
        .iter()
        .map(|r| format!("{}:{} {}", r.file, r.start_line, r.name).len())
        .max()
        .unwrap_or(20)
        .max(8);

    println!(
        "{:>6} | {:>4} | {:>6} | {:<width$}",
        "CRAP",
        "CC",
        "Cov%",
        "Function",
        width = name_width,
    );
    println!(
        "{:-<6}-+-{:-<4}-+-{:-<6}-+-{:-<width$}",
        "",
        "",
        "",
        "",
        width = name_width,
    );

    for r in display {
        let location = format!("{}:{}", r.file, r.start_line);
        println!(
            "{:>6.1} | {:>4} | {:>5.1}% | {location} {}",
            r.crap_score, r.complexity, r.coverage_pct, r.name,
        );
    }

    println!();
    println!("Total functions: {}", records.len());

    if let Some(threshold) = opts.threshold {
        let above = records.iter().filter(|r| r.crap_score > threshold).count();
        println!("Functions above CRAP threshold ({threshold:.0}): {above}");
    }
}
