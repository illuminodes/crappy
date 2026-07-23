#[macro_use]
mod color;
mod complexity;
mod coverage;
mod dryness;
mod idiom;
mod report;
mod scoring;
mod visitor;

use std::fmt;
use std::path::PathBuf;
use std::time::Instant;

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
    Json(json_bourne::Error),
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

impl From<json_bourne::Error> for Error {
    fn from(e: json_bourne::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Default)]
pub(crate) struct Opts {
    pub threshold: Option<f64>,
    pub top: Option<usize>,
    pub package: Option<String>,
    pub exclude_paths: Vec<String>,
    pub exclude_fns: Vec<String>,
    pub features: Vec<String>,
    pub all_features: bool,
    pub no_default_features: bool,
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
            "-p" | "--package" => {
                let val = parse_flag_value(args, &mut i, "--package")?;
                opts.package = Some(val.to_string());
            }
            "--features" => {
                let val = parse_flag_value(args, &mut i, "--features")?;
                opts.features.push(val.to_string());
            }
            "--all-features" => opts.all_features = true,
            "--no-default-features" => opts.no_default_features = true,
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
cargo-crappy — CRAP metric analysis with idiomatic Rust scoring

USAGE:
    cargo crappy [OPTIONS]

OPTIONS:
    -p, --package <SPEC>   Analyze only the specified package (in a workspace)
    --threshold <N>        Exit with code 1 if any function exceeds this CRAPPY score
    --top <N>              Show only the top N worst functions
    --exclude-path <PAT>   Exclude functions whose file path contains PAT (repeatable)
    --exclude-fn <NAME>    Exclude a function by name (repeatable)
    --features <F>         Comma-separated features to activate (passed to cargo test)
    --all-features         Activate all available features
    --no-default-features  Do not activate the `default` feature
    -h, --help             Print help
    -V, --version          Print version

SCORING:
    CRAPPY = CRAP x idiom_penalty

    CRAP (Change Risk Anti-Patterns):
        CRAP = CC^2 x (1 - cov/100)^3 + CC
        CC   = cyclomatic complexity (branches, match arms, loops, &&/||, ?)
        cov  = line coverage percentage from instrumented tests
        A fully-covered function scores CRAPPY = CC (complexity alone).
        An uncovered function scores CRAPPY = CC^2 + CC (risk amplified).

    Idiom penalty (multiplier >= 1.0):
        penalty = 1.0 + demerits x 0.25
        Each violation adds demerits; the penalty multiplies the CRAP score.
        Clean code gets 1.0x (no change). A single high-weight violation
        adds 50%. Multiple violations stack.

    Idiom checks (high-weight, 2 demerits each):
        - Free function whose first param is &Struct (should be a method)
        - Match on integer/string/char literals (should be an enum)
        - Primitive `as` cast inside comparison/arithmetic (use Ord/Add traits)

    Idiom checks (low-weight, 1 demerit each):
        - .unwrap() calls (use ? or .expect())
        - Explicit drop() calls (use scoped blocks)
        - vec![] for empty vectors (use Vec::new())
        - FromIterator::from_iter() (use .collect())
        - Box<dyn Error> in return types (use concrete error types)

    DRY-ness checks (3 demerits each):
        - Signature duplicate: two functions with identical (param_types)->return_type
        - Body duplicate: two functions with identical normalized AST structure
        Both can stack (6 demerits) when a function matches on both signals.

SUPPRESSION:
    #[allow(crappy)]   Annotate a function to exclude it from analysis.
                       Use #[allow(unknown_lints, crappy)] to also silence the compiler warning.

OUTPUT:
    Each function gets a clippy-style diagnostic with its location
    and actionable suggestions based on what contributes to its score.
    Functions above the threshold (default 30) are shown as warnings."
    );
}

pub(crate) fn cargo_feature_args(opts: &Opts) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(pkg) = &opts.package {
        args.push("--package".to_string());
        args.push(pkg.clone());
    }
    for f in &opts.features {
        args.push("--features".to_string());
        args.push(f.clone());
    }
    if opts.all_features {
        args.push("--all-features".to_string());
    }
    if opts.no_default_features {
        args.push("--no-default-features".to_string());
    }
    args
}

pub(crate) fn cargo_metadata_args(opts: &Opts) -> Vec<String> {
    let mut args = Vec::new();
    for f in &opts.features {
        args.push("--features".to_string());
        args.push(f.clone());
    }
    if opts.all_features {
        args.push("--all-features".to_string());
    }
    if opts.no_default_features {
        args.push("--no-default-features".to_string());
    }
    args
}

fn format_duration(d: std::time::Duration) -> String {
    let total_secs = d.as_secs();
    let millis = d.subsec_millis();
    if total_secs < 60 {
        format!("{total_secs}.{millis:03}s")
    } else {
        let mins = total_secs / 60;
        let rem = total_secs % 60;
        format!("{mins}m {rem}.{millis:03}s")
    }
}

pub(crate) fn analyze(
    project_dir: &std::path::Path,
    opts: &Opts,
) -> Result<Vec<scoring::CrapRecord>, Error> {
    let start = Instant::now();
    let cargo_args = cargo_feature_args(opts);
    let metadata_args = cargo_metadata_args(opts);

    eprintln!(
        "{} tests with coverage instrumentation...",
        color::bold_green("Instrumenting")
    );
    let cov = coverage::collect_coverage(project_dir, &cargo_args)?;
    eprintln!(
        "    {} {} functions with coverage data",
        color::bold_green("Collected"),
        cov.len()
    );

    eprintln!(
        "    {} complexity and idioms...",
        color::bold_green("Analyzing")
    );
    let (comp, idioms) =
        complexity::analyze_all(project_dir, &metadata_args, opts.package.as_deref())?;
    eprintln!(
        "    {} {} functions",
        color::bold_green("Analyzed"),
        comp.len()
    );

    let mut records = scoring::compute_crap_scores(cov, &comp, idioms, project_dir);

    records.retain(|r| {
        !opts
            .exclude_paths
            .iter()
            .any(|p| r.file.contains(p.as_str()))
            && !opts.exclude_fns.contains(&r.name)
    });

    eprintln!(
        "    {} {} functions in {}\n",
        color::bold_green("Finished"),
        records.len(),
        format_duration(start.elapsed()),
    );

    report::print_report(&records, opts);

    Ok(records)
}

fn detect_package(project_dir: &std::path::Path) -> Option<String> {
    let manifest = project_dir.join("Cargo.toml");
    let content = std::fs::read_to_string(&manifest).ok()?;

    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && trimmed.starts_with("name") {
            let (_, val) = trimmed.split_once('=')?;
            let val = val.trim().trim_matches('"');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn is_workspace_member(project_dir: &std::path::Path) -> bool {
    let mut dir = project_dir.parent();
    while let Some(d) = dir {
        let manifest = d.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&manifest)
            && content.contains("[workspace]")
        {
            return true;
        }
        dir = d.parent();
    }
    false
}

#[allow(unknown_lints, crappy)]
fn run() -> Result<(), Error> {
    let mut opts = match parse_args()? {
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

    if opts.package.is_none() && is_workspace_member(&project_dir) {
        opts.package = detect_package(&project_dir);
    }
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
    fn features_flag() {
        let a = args(&["--features", "foo,bar"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert_eq!(opts.features, vec!["foo,bar"]);
    }

    #[test]
    fn all_features_flag() {
        let a = args(&["--all-features"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert!(opts.all_features);
    }

    #[test]
    fn no_default_features_flag() {
        let a = args(&["--no-default-features"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert!(opts.no_default_features);
    }

    #[test]
    fn cargo_feature_args_empty() {
        let opts = Opts::default();
        assert!(cargo_feature_args(&opts).is_empty());
    }

    #[test]
    fn cargo_feature_args_all() {
        let opts = Opts {
            features: vec!["foo".into(), "bar".into()],
            all_features: true,
            no_default_features: true,
            ..Opts::default()
        };
        let a = cargo_feature_args(&opts);
        assert!(a.contains(&"--features".to_string()));
        assert!(a.contains(&"foo".to_string()));
        assert!(a.contains(&"bar".to_string()));
        assert!(a.contains(&"--all-features".to_string()));
        assert!(a.contains(&"--no-default-features".to_string()));
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
        let err = json_bourne::parse::<bool>(b"not json").unwrap_err();
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
        let b_err = json_bourne::parse::<bool>(b"bad").unwrap_err();
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

        let cov = coverage::collect_coverage(&dir, &[]).unwrap();
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

    #[test]
    fn package_flag_short() {
        let a = args(&["-p", "my-crate"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert_eq!(opts.package.as_deref(), Some("my-crate"));
    }

    #[test]
    fn package_flag_long() {
        let a = args(&["--package", "my-crate"]);
        let Action::Run(opts) = parse_args_from(&a).unwrap() else {
            panic!("expected Run");
        };
        assert_eq!(opts.package.as_deref(), Some("my-crate"));
    }

    #[test]
    fn cargo_feature_args_with_package() {
        let opts = Opts {
            package: Some("my-crate".into()),
            ..Opts::default()
        };
        let a = cargo_feature_args(&opts);
        assert_eq!(a, vec!["--package", "my-crate"]);
    }

    #[test]
    fn detect_package_finds_name() {
        let dir = std::env::temp_dir().join(format!("crappy-detect-{}-find", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(detect_package(&dir), Some("my-crate".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_package_returns_none_for_workspace_root() {
        let dir = std::env::temp_dir().join(format!("crappy-detect-{}-ws", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        assert_eq!(detect_package(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_workspace_member_detects_parent_workspace() {
        let dir = std::env::temp_dir().join(format!("crappy-ws-{}-member", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let member = dir.join("crates").join("foo");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert!(is_workspace_member(&member));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_workspace_member_false_for_standalone() {
        let dir = std::env::temp_dir().join(format!("crappy-ws-{}-standalone", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert!(!is_workspace_member(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
