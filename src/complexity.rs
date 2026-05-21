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

        let start_line = sig.ident.span().start().line as u32;

        let mut counter = BranchCounter { count: 1 };
        counter.visit_block(block);

        let end_line = sig.ident.span().end().line as u32;
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

fn block_end_line(block: &syn::Block) -> u32 {
    block.brace_token.span.close().end().line as u32
}

fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("test"))
}

fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
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

fn format_type(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(tp) => format_path(&tp.path),
        _ => "_".to_string(),
    }
}

fn format_path(path: &syn::Path) -> String {
    path.segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_else(|| "_".to_string())
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

pub fn analyze_complexity(project_dir: &Path) -> Result<Vec<FunctionComplexity>, Error> {
    let src_dir = project_dir.join("src");
    if !src_dir.exists() {
        return Ok(Vec::new());
    }

    let mut rs_files = Vec::new();
    collect_rs_files(&src_dir, &mut rs_files)?;

    let mut all_functions = Vec::new();

    for file_path in &rs_files {
        let source = fs::read_to_string(file_path)?;
        let syntax = syn::parse_file(&source).map_err(|e| Error::Syn {
            file: file_path.clone(),
            error: e,
        })?;

        let mut visitor = FileVisitor {
            file: file_path.canonicalize()?,
            context: Vec::new(),
            functions: Vec::new(),
        };
        visitor.visit_file(&syntax);
        all_functions.extend(visitor.functions);
    }

    Ok(all_functions)
}
