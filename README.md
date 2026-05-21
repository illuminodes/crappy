# cargo-crappy

CRAP metric analysis for Rust — clippy-style diagnostics for change-risk, complexity, coverage, and idiomatic code.

## Install

```sh
cargo install cargo-crappy
```

Requires the `llvm-tools` component:

```sh
rustup component add llvm-tools
```

## Usage

```sh
cargo crappy
```

```
warning[high-risk]: `parse_input` has high change-risk score
 --> src/parser.rs:42
  |
  = note: CRAPPY = 156.0, CRAP = 156.0, CC = 12, Cov = 0%
  = help: coverage is 0% — add tests to reduce risk
  = help: cyclomatic complexity is 12 — consider splitting into smaller functions

warning: `cargo crappy` generated 3 warnings
```

### Options

```
--threshold <N>        Exit with code 1 if any function exceeds this CRAPPY score (default: 30)
--top <N>              Show the top N worst functions regardless of threshold
--exclude-path <PAT>   Exclude functions whose file path contains PAT (repeatable)
--exclude-fn <NAME>    Exclude a function by name (repeatable)
--features <F>         Comma-separated features to activate (passed to cargo test)
--all-features         Activate all available features
--no-default-features  Do not activate the `default` feature
```

### CI example

```sh
cargo crappy --threshold 30
```

Exits with code 1 if any function exceeds the threshold.

## Suppression

Annotate a function to exclude it from analysis:

```rust
#[allow(unknown_lints, crappy)]
fn intentionally_complex() {
    // ...
}
```

## Scoring

**CRAPPY = CRAP x idiom_penalty**

### CRAP (Change Risk Anti-Patterns)

```
CRAP = CC^2 x (1 - cov/100)^3 + CC
```

- **CC** — cyclomatic complexity (branches, match arms, loops, `&&`/`||`, `?`)
- **cov** — line coverage percentage from instrumented tests
- A fully-covered function scores `CRAPPY = CC` (complexity alone)
- An uncovered function scores `CRAPPY = CC^2 + CC` (risk amplified)

### Idiom penalty

```
penalty = 1.0 + demerits x 0.25
```

Each violation adds demerits; the penalty multiplies the CRAP score.

**High-weight (2 demerits each):**
- Free function whose first param is `&Struct` (should be a method)
- Match on integer/string/char literals (should be an enum)
- Primitive `as` cast in comparison/arithmetic (use `From`/`Into` traits)

**Low-weight (1 demerit each):**
- `.unwrap()` calls (use `?` or `.expect()`)
- Explicit `drop()` calls (use scoped blocks)
- `vec![]` for empty vectors (use `Vec::new()`)
- `FromIterator::from_iter()` (use `.collect()`)
- `Box<dyn Error>` in return types (use concrete error types)

### DRY-ness (3 demerits each)

- **Signature duplicate** — two functions with identical `(param_types) -> return_type`
- **Body duplicate** — two functions with identical normalized AST structure

Both can stack (6 demerits) when a function matches on both signals.

## License

MIT
