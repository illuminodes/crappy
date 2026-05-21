mod complexity;
mod coverage;
mod idiom;
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

#[derive(Default)]
pub(crate) struct Opts {
    pub threshold: Option<f64>,
    pub top: Option<usize>,
    pub exclude_paths: Vec<String>,
    pub exclude_fns: Vec<String>,
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
        .map(std::string::String::as_str)
        .ok_or_else(|| arg_error(&format!("{name} requires a value")))
}

fn parse_args_from(args: &[String]) -> Result<Action, Error> {
    let mut opts = Opts::default();

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
            "--exclude-path" => {
                let val = parse_flag_value(args, &mut i, "--exclude-path")?;
                opts.exclude_paths.push(val.to_string());
            }
            "--exclude-fn" => {
                let val = parse_flag_value(args, &mut i, "--exclude-fn")?;
                opts.exclude_fns.push(val.to_string());
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
    --threshold <N>        Exit with code 1 if any function exceeds this CRAPPY score
    --top <N>              Show only the top N worst functions
    --exclude-path <PAT>   Exclude functions whose file path contains PAT (repeatable)
    --exclude-fn <NAME>    Exclude a function by name (repeatable)
    -h, --help             Print help
    -V, --version          Print version"
    );
}

pub(crate) fn analyze(
    project_dir: &std::path::Path,
    opts: &Opts,
) -> Result<Vec<scoring::CrapRecord>, Error> {
    eprintln!("Running tests with coverage instrumentation...");
    let cov = coverage::collect_coverage(project_dir)?;
    eprintln!("  {} functions with coverage data", cov.len());

    eprintln!("Analyzing complexity and idioms...");
    let (comp, idioms) = complexity::analyze_all(project_dir)?;
    eprintln!("  {} functions analyzed", comp.len());

    let mut records = scoring::compute_crap_scores(cov, &comp, idioms, project_dir);

    records.retain(|r| {
        !opts
            .exclude_paths
            .iter()
            .any(|p| r.file.contains(p.as_str()))
            && !opts.exclude_fns.contains(&r.name)
    });

    eprintln!("  {} functions scored\n", records.len());

    report::print_report(&records, opts);

    Ok(records)
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
    let records = analyze(&project_dir, &opts)?;

    if let Some(threshold) = opts.threshold
        && records.iter().any(|r| r.crappy_score > threshold)
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
        strs.iter().map(|s| (*s).to_string()).collect()
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

    #[test]
    fn exclude_path_flag() {
        let a = args(&["--exclude-path", "tests/", "--exclude-path", "benches/"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert_eq!(opts.exclude_paths, vec!["tests/", "benches/"]);
    }

    #[test]
    fn exclude_fn_flag() {
        let a = args(&["--exclude-fn", "main", "--exclude-fn", "run"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert_eq!(opts.exclude_fns, vec!["main", "run"]);
    }

    #[test]
    fn exclude_combined_with_threshold() {
        let a = args(&[
            "--threshold",
            "30",
            "--exclude-path",
            "tests/",
            "--exclude-fn",
            "run",
        ]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert!((opts.threshold.unwrap() - 30.0).abs() < f64::EPSILON);
        assert_eq!(opts.exclude_paths, vec!["tests/"]);
        assert_eq!(opts.exclude_fns, vec!["run"]);
    }

    #[test]
    fn display_command_error() {
        use std::os::unix::process::ExitStatusExt;
        let e = Error::Command {
            tool: "cargo test",
            status: std::process::ExitStatus::from_raw(256), // exit code 1
        };
        let msg = format!("{e}");
        assert!(msg.contains("cargo test"));
    }

    #[test]
    fn display_tool_not_found() {
        let e = Error::ToolNotFound {
            tool: "llvm-cov".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("llvm-cov"));
        assert!(msg.contains("rustup component add llvm-tools"));
    }

    #[test]
    fn display_io_error() {
        let e = Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert!(format!("{e}").contains("gone"));
    }

    #[test]
    fn display_json_error() {
        let err = bourne::parse::<bool>(b"not json").unwrap_err();
        let e = Error::Json(err);
        assert!(!format!("{e}").is_empty());
    }

    #[test]
    fn display_syn_error() {
        let err = syn::parse_file("fn {")
            .map(|_| ())
            .expect_err("should fail");
        let e = Error::Syn {
            file: PathBuf::from("test.rs"),
            error: err,
        };
        let msg = format!("{e}");
        assert!(msg.contains("test.rs"));
    }

    #[test]
    fn display_no_test_binaries() {
        assert!(format!("{}", Error::NoTestBinaries).contains("no test binaries"));
    }

    #[test]
    fn display_no_profraw() {
        assert!(format!("{}", Error::NoProfrawFiles).contains("profraw"));
    }

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::other("x");
        let e: Error = io_err.into();
        assert!(matches!(e, Error::Io(_)));
    }

    #[test]
    fn from_bourne_error() {
        let b_err = bourne::parse::<bool>(b"bad").unwrap_err();
        let e: Error = b_err.into();
        assert!(matches!(e, Error::Json(_)));
    }

    fn create_temp_project(name: &str, lib_rs: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crappy-inproc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"test-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(src.join("lib.rs"), lib_rs).unwrap();
        dir
    }

    #[test]
    #[ignore = "spawns cargo test — conflicts with outer coverage instrumentation"]
    fn collect_coverage_on_temp_project() {
        let dir = create_temp_project(
            "cov",
            r"
pub fn add(a: i32, b: i32) -> i32 { a + b }
pub fn unused(x: i32) -> i32 { if x > 0 { x } else { -x } }
#[cfg(test)]
mod tests {
    #[test]
    fn test_add() { assert_eq!(super::add(1, 2), 3); }
}
",
        );

        let cov = coverage::collect_coverage(&dir).unwrap();
        assert!(!cov.is_empty(), "should find covered functions");

        let add_cov = cov
            .iter()
            .find(|f| f.file.to_str().unwrap_or("").contains("lib.rs") && f.start_line <= 2);
        assert!(add_cov.is_some(), "should find coverage for add");
        assert!(
            add_cov.unwrap().line_coverage_pct > 0.0,
            "add should have coverage"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "spawns cargo test — conflicts with outer coverage instrumentation"]
    fn analyze_full_pipeline() {
        let dir = create_temp_project(
            "full",
            r"
pub fn covered() -> i32 { 42 }
pub fn branchy(x: bool) -> i32 { if x { 1 } else { 2 } }
#[cfg(test)]
mod tests {
    #[test]
    fn test_covered() { assert_eq!(super::covered(), 42); }
}
",
        );

        let records = analyze(&dir, &Opts::default()).unwrap();
        assert_eq!(records.len(), 2, "should score 2 functions");

        let covered = records.iter().find(|r| r.name == "covered").unwrap();
        assert_eq!(covered.complexity, 1);
        assert!(covered.coverage_pct > 0.0);
        assert!((covered.crap_score - 1.0).abs() < 0.1);

        let branchy = records.iter().find(|r| r.name == "branchy").unwrap();
        assert!(branchy.complexity >= 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
