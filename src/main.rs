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

pub(crate) struct Opts {
    pub threshold: Option<f64>,
    pub top: Option<usize>,
}

enum Action {
    Run(Opts),
    Help,
    Version,
}

fn arg_error(msg: &str) -> Error {
    Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, msg))
}

fn parse_flag_value<'a>(args: &'a [String], i: &mut usize, name: &str) -> Result<&'a str, Error> {
    *i += 1;
    args.get(*i)
        .map(|s| s.as_str())
        .ok_or_else(|| arg_error(&format!("{name} requires a value")))
}

fn parse_args_from(args: &[String]) -> Result<Action, Error> {
    let mut opts = Opts {
        threshold: None,
        top: None,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--threshold" => {
                let val = parse_flag_value(args, &mut i, "--threshold")?;
                opts.threshold = Some(
                    val.parse::<f64>()
                        .map_err(|_| arg_error("invalid threshold value"))?,
                );
            }
            "--top" => {
                let val = parse_flag_value(args, &mut i, "--top")?;
                opts.top = Some(
                    val.parse::<usize>()
                        .map_err(|_| arg_error("invalid top value"))?,
                );
            }
            "-h" | "--help" => return Ok(Action::Help),
            "-V" | "--version" => return Ok(Action::Version),
            other => {
                eprintln!("unknown option: {other}");
                return Ok(Action::Help);
            }
        }
        i += 1;
    }

    Ok(Action::Run(opts))
}

fn parse_args() -> Result<Action, Error> {
    let args: Vec<String> = std::env::args().collect();
    let args = if args.get(1).is_some_and(|s| s == "crappy") {
        &args[2..]
    } else {
        &args[1..]
    };
    parse_args_from(args)
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
    let opts = match parse_args()? {
        Action::Run(opts) => opts,
        Action::Help => {
            print_help();
            return Ok(());
        }
        Action::Version => {
            println!("cargo-crappy {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
    };

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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args() {
        let a = args(&[]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert!(opts.threshold.is_none());
        assert!(opts.top.is_none());
    }

    #[test]
    fn threshold_flag() {
        let a = args(&["--threshold", "30"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert!((opts.threshold.unwrap() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn top_flag() {
        let a = args(&["--top", "5"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert_eq!(opts.top.unwrap(), 5);
    }

    #[test]
    fn both_flags() {
        let a = args(&["--threshold", "20", "--top", "10"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert!((opts.threshold.unwrap() - 20.0).abs() < f64::EPSILON);
        assert_eq!(opts.top.unwrap(), 10);
    }

    #[test]
    fn help_flag() {
        let a = args(&["--help"]);
        assert!(matches!(parse_args_from(&a).unwrap(), Action::Help));
    }

    #[test]
    fn version_flag() {
        let a = args(&["-V"]);
        assert!(matches!(parse_args_from(&a).unwrap(), Action::Version));
    }

    #[test]
    fn missing_threshold_value() {
        let a = args(&["--threshold"]);
        assert!(parse_args_from(&a).is_err());
    }

    #[test]
    fn invalid_threshold_value() {
        let a = args(&["--threshold", "abc"]);
        assert!(parse_args_from(&a).is_err());
    }
}
