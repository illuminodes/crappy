use std::fs;
use std::path::{Path, PathBuf};

use syn::BinOp;
use syn::visit::Visit;

use crate::Error;

pub struct FunctionComplexity {
    pub file: PathBuf,
    pub qualified_name: String,
    pub start_line: u32,
    pub end_line: u32,
    pub complexity: u32,
}

struct FileVisitor {
    file: PathBuf,
    context: Vec<String>,
    functions: Vec<FunctionComplexity>,
}

impl<'ast> Visit<'ast> for FileVisitor {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if has_test_attr(&node.attrs) {
            return;
        }
        let name = node.sig.ident.to_string();
        self.record_function(&name, &node.block, &node.sig);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let ctx = if let Some((_, path, _)) = &node.trait_ {
            format_path(path)
        } else {
            format_type(&node.self_ty)
        };
        self.context.push(ctx);
        syn::visit::visit_item_impl(self, node);
        self.context.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if has_test_attr(&node.attrs) {
            return;
        }
        let name = node.sig.ident.to_string();
        self.record_function(&name, &node.block, &node.sig);
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.context.push(node.ident.to_string());
        syn::visit::visit_item_trait(self, node);
        self.context.pop();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if let Some(block) = &node.default
            && !has_test_attr(&node.attrs)
        {
            let name = node.sig.ident.to_string();
            self.record_function(&name, block, &node.sig);
        }
        syn::visit::visit_trait_item_fn(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }
}

impl FileVisitor {
    fn record_function(&mut self, name: &str, block: &syn::Block, sig: &syn::Signature) {
        let qualified = if self.context.is_empty() {
            name.to_string()
        } else {
            format!("{}::{name}", self.context.last().unwrap())
        };

        let start_line = line_u32(sig.ident.span().start().line);

        let mut counter = BranchCounter { count: 1 };
        counter.visit_block(block);

        let end_line = line_u32(sig.ident.span().end().line);
        let block_end = block_end_line(block);
        let end = end_line.max(block_end);

        self.functions.push(FunctionComplexity {
            file: self.file.clone(),
            qualified_name: qualified,
            start_line,
            end_line: end,
            complexity: counter.count,
        });
    }
}

struct BranchCounter {
    count: u32,
}

impl<'ast> Visit<'ast> for BranchCounter {
    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        self.count += 1;
        syn::visit::visit_expr_if(self, node);
    }

    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        self.count += 1;
        syn::visit::visit_arm(self, node);
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.count += 1;
        syn::visit::visit_expr_while(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.count += 1;
        syn::visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.count += 1;
        syn::visit::visit_expr_loop(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        match node.op {
            BinOp::And(_) | BinOp::Or(_) => self.count += 1,
            _ => {}
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        self.count += 1;
        syn::visit::visit_expr_try(self, node);
    }

    fn visit_expr_closure(&mut self, _node: &'ast syn::ExprClosure) {
        // Don't recurse into closures — they're separate callable units.
    }
}

fn line_u32(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

fn block_end_line(block: &syn::Block) -> u32 {
    line_u32(block.brace_token.span.close().end().line)
}

pub fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("test"))
}

pub fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let Ok(nested) = a.parse_args::<syn::Ident>() else {
            return false;
        };
        nested == "test"
    })
}

pub fn format_type(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(tp) => format_path(&tp.path),
        _ => "_".to_string(),
    }
}

pub fn format_path(path: &syn::Path) -> String {
    path.segments
        .last()
        .map_or_else(|| "_".to_string(), |s| s.ident.to_string())
}

fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default();
            if name == "target" || name == ".git" {
                continue;
            }
            collect_rs_files(&path, files)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
fn analyze_source(source: &str) -> Vec<(String, u32)> {
    let syntax = syn::parse_file(source).expect("test source must parse");
    let mut visitor = FileVisitor {
        file: PathBuf::from("test.rs"),
        context: Vec::new(),
        functions: Vec::new(),
    };
    visitor.visit_file(&syntax);
    visitor
        .functions
        .into_iter()
        .map(|f| (f.qualified_name, f.complexity))
        .collect()
}

fn find_source_dirs(project_dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(project_dir)
        .output()?;

    if !output.status.success() {
        return Ok(vec![project_dir.join("src")]);
    }

    bourne::from_json! {
        #[bourne(deny_unknown_fields = false)]
        struct Meta {
            packages: Vec<MetaPkg>,
        }
    }

    bourne::from_json! {
        #[bourne(deny_unknown_fields = false)]
        struct MetaPkg {
            targets: Vec<MetaTarget>,
        }
    }

    bourne::from_json! {
        #[bourne(deny_unknown_fields = false)]
        struct MetaTarget {
            kind: Vec<String>,
            src_path: String,
        }
    }

    let Ok(meta) = bourne::parse::<Meta>(&output.stdout) else {
        return Ok(vec![project_dir.join("src")]);
    };

    let mut dirs = Vec::new();
    for pkg in &meta.packages {
        for target in &pkg.targets {
            if target.kind.iter().any(|k| k == "lib" || k == "bin") {
                let src_path = PathBuf::from(&target.src_path);
                if let Some(parent) = src_path.parent()
                    && parent.exists() && !dirs.contains(&parent.to_path_buf()) {
                        dirs.push(parent.to_path_buf());
                    }
            }
        }
    }

    if dirs.is_empty() {
        dirs.push(project_dir.join("src"));
    }

    Ok(dirs)
}

pub fn analyze_all(
    project_dir: &Path,
) -> Result<(Vec<FunctionComplexity>, Vec<crate::idiom::FunctionIdioms>), Error> {
    let source_dirs = find_source_dirs(project_dir)?;

    let mut rs_files = Vec::new();
    for src_dir in &source_dirs {
        if src_dir.exists() {
            collect_rs_files(src_dir, &mut rs_files)?;
        }
    }

    let mut all_complexity = Vec::new();
    let mut all_idioms = Vec::new();

    for file_path in &rs_files {
        let source = fs::read_to_string(file_path)?;
        let syntax = syn::parse_file(&source).map_err(|e| Error::Syn {
            file: file_path.clone(),
            error: e,
        })?;

        let canonical = file_path.canonicalize()?;

        let mut visitor = FileVisitor {
            file: canonical.clone(),
            context: Vec::new(),
            functions: Vec::new(),
        };
        visitor.visit_file(&syntax);
        all_complexity.extend(visitor.functions);

        let idiom_results = crate::idiom::analyze_idioms_for_file(&canonical, &syntax);
        all_idioms.extend(idiom_results);
    }

    Ok((all_complexity, all_idioms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(source: &str) -> Vec<(String, u32)> {
        analyze_source(source)
    }

    #[test]
    fn trivial_function() {
        let r = cc("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert_eq!(r, vec![("add".into(), 1)]);
    }

    #[test]
    fn single_if() {
        let r = cc("fn f(x: bool) { if x { } }");
        assert_eq!(r[0].1, 2);
    }

    #[test]
    fn if_else() {
        let r = cc("fn f(x: bool) { if x { } else { } }");
        assert_eq!(r[0].1, 2);
    }

    #[test]
    fn nested_if() {
        let r = cc("fn f(a: bool, b: bool) { if a { if b { } } }");
        assert_eq!(r[0].1, 3);
    }

    #[test]
    fn match_arms() {
        let r = cc("fn f(x: i32) { match x { 1 => {}, 2 => {}, _ => {} } }");
        assert_eq!(r[0].1, 4); // 1 base + 3 arms
    }

    #[test]
    fn while_loop() {
        let r = cc("fn f() { while true { } }");
        assert_eq!(r[0].1, 2);
    }

    #[test]
    fn for_loop() {
        let r = cc("fn f() { for _ in 0..10 { } }");
        assert_eq!(r[0].1, 2);
    }

    #[test]
    fn infinite_loop() {
        let r = cc("fn f() { loop { break; } }");
        assert_eq!(r[0].1, 2);
    }

    #[test]
    fn logical_and_or() {
        let r = cc("fn f(a: bool, b: bool, c: bool) -> bool { a && b || c }");
        assert_eq!(r[0].1, 3); // 1 base + && + ||
    }

    #[test]
    fn try_operator() {
        let r = cc("fn f() -> Result<(), ()> { let _ = Ok::<(), ()>(()).map(|_| ())?; Ok(()) }");
        assert_eq!(r[0].1, 2); // 1 base + ?
    }

    #[test]
    fn closure_not_counted() {
        let r = cc("fn f() { let _ = |x: bool| { if x { 1 } else { 2 } }; }");
        assert_eq!(r[0].1, 1); // closure's if not counted in f
    }

    #[test]
    fn impl_method_qualified() {
        let r = cc("struct S; impl S { fn method(&self) {} }");
        assert_eq!(r[0].0, "S::method");
        assert_eq!(r[0].1, 1);
    }

    #[test]
    fn trait_default_method() {
        let r = cc("trait T { fn default_method(&self) { if true {} } }");
        assert_eq!(r[0].0, "T::default_method");
        assert_eq!(r[0].1, 2);
    }

    #[test]
    fn test_functions_skipped() {
        let r = cc("#[test] fn test_something() { if true {} }");
        assert!(r.is_empty());
    }

    #[test]
    fn cfg_test_module_skipped() {
        let r = cc("fn visible() {} #[cfg(test)] mod tests { fn hidden() {} }");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "visible");
    }

    #[test]
    fn combined_complexity() {
        let r = cc("fn complex(x: i32, flag: bool) -> Result<(), ()> {
                if flag && x > 0 {
                    match x {
                        1 => {},
                        2 => {},
                        _ => {},
                    }
                }
                for _ in 0..x {
                    let _ = Ok::<(), ()>(())?;
                }
                Ok(())
            }");
        // 1 base + if + && + 3 arms + for + ?
        assert_eq!(r[0].1, 8);
    }

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crappy-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collect_rs_files_finds_nested() {
        let dir = tmpdir("nested");
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.join("a.rs"), "").unwrap();
        fs::write(sub.join("b.rs"), "").unwrap();
        fs::write(dir.join("c.txt"), "").unwrap();

        let mut files = Vec::new();
        collect_rs_files(&dir, &mut files).unwrap();
        files.sort();

        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("a.rs"));
        assert!(files[1].ends_with("b.rs"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_rs_files_skips_target_and_git() {
        let dir = tmpdir("skip");
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("target/bad.rs"), "").unwrap();
        fs::write(dir.join(".git/bad.rs"), "").unwrap();
        fs::write(dir.join("good.rs"), "").unwrap();

        let mut files = Vec::new();
        collect_rs_files(&dir, &mut files).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("good.rs"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyze_complexity_on_temp_project() {
        let dir = tmpdir("proj");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("lib.rs"),
            "fn simple() {} fn branchy(x: bool) { if x {} }",
        )
        .unwrap();

        let result = analyze_all(&dir).map(|(c, _)| c).unwrap();
        assert_eq!(result.len(), 2);

        let simple = result
            .iter()
            .find(|f| f.qualified_name == "simple")
            .unwrap();
        assert_eq!(simple.complexity, 1);

        let branchy = result
            .iter()
            .find(|f| f.qualified_name == "branchy")
            .unwrap();
        assert_eq!(branchy.complexity, 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyze_complexity_no_src_dir() {
        let dir = tmpdir("nosrc");
        let result = analyze_all(&dir).map(|(c, _)| c).unwrap();
        assert!(result.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
