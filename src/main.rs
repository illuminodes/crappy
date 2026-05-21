mod complexity;
mod coverage;
mod report;
mod scoring;

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    Command {
        tool: &'static str,
        status: std::process::ExitStatus,
    },
    ToolNotFound {
        tool: String,
    },
    Io(std::io::Error),
    Json(bourne::Error),
    Syn {
        file: PathBuf,
        error: syn::Error,
    },
    NoTestBinaries,
    NoProfrawFiles,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command { tool, status } => write!(f, "{tool} exited with {status}"),
            Self::ToolNotFound { tool } => {
                write!(f, "{tool} not found (run: rustup component add llvm-tools)")
            }
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "JSON parse error: {e}"),
            Self::Syn { file, error } => write!(f, "parse error in {}: {error}", file.display()),
            Self::NoTestBinaries => write!(f, "no test binaries found"),
            Self::NoProfrawFiles => write!(f, "no .profraw files generated (did any tests run?)"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<bourne::Error> for Error {
    fn from(e: bourne::Error) -> Self {
        Self::Json(e)
    }
}

struct Opts {
    threshold: Option<f64>,
    top: Option<usize>,
}

fn parse_args() -> Result<Opts, Error> {
    let args: Vec<String> = std::env::args().collect();
    let args = if args.get(1).is_some_and(|s| s == "crappy") {
        &args[2..]
    } else {
        &args[1..]
    };

    let mut opts = Opts {
        threshold: None,
        top: None,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--threshold" => {
                i += 1;
                let val = args.get(i).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "--threshold requires a value",
                    )
                })?;
                opts.threshold = Some(val.parse::<f64>().map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid threshold value")
                })?);
            }
            "--top" => {
                i += 1;
                let val = args.get(i).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "--top requires a value")
                })?;
                opts.top = Some(val.parse::<usize>().map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid top value")
                })?);
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("cargo-crappy {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown option: {other}");
                print_help();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    Ok(opts)
}

fn print_help() {
    eprintln!(
        "\
cargo-crappy — CRAP metric analysis for Rust

USAGE:
    cargo crappy [OPTIONS]

OPTIONS:
    --threshold <N>   Exit with code 1 if any function exceeds this CRAP score
    --top <N>         Show only the top N worst functions
    -h, --help        Print help
    -V, --version     Print version"
    );
}

fn run() -> Result<(), Error> {
    let opts = parse_args()?;
    let project_dir = std::env::current_dir()?;

    eprintln!("Running tests with coverage instrumentation...");
    let cov = coverage::collect_coverage(&project_dir)?;
    eprintln!("  {} functions with coverage data", cov.len());

    eprintln!("Analyzing cyclomatic complexity...");
    let comp = complexity::analyze_complexity(&project_dir)?;
    eprintln!("  {} functions analyzed", comp.len());

    let records = scoring::compute_crap_scores(cov, comp, &project_dir);
    eprintln!("  {} functions scored\n", records.len());

    report::print_report(&records, &opts);

    if let Some(threshold) = opts.threshold
        && records.iter().any(|r| r.crap_score > threshold)
    {
        std::process::exit(1);
    }

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(2);
    }
}
