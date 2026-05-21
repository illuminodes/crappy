use std::collections::HashSet;
use std::path::PathBuf;

use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{BinOp, Expr, FnArg, Item, Pat, ReturnType, Type};

use crate::complexity::{has_cfg_test, has_test_attr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdiomCheck {
    FreeMethodCandidate,
    MatchOnLiteral,
    PrimitiveCastInComparison,
    Unwrap,
    ExplicitDrop,
    EmptyVecMacro,
    FromIterInsteadOfCollect,
    BoxDynError,
}

impl IdiomCheck {
    pub fn weight(self) -> u32 {
        match self {
            Self::FreeMethodCandidate | Self::MatchOnLiteral | Self::PrimitiveCastInComparison => 2,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdiomViolation {
    pub check: IdiomCheck,
    pub line: u32,
}

pub struct FunctionIdioms {
    pub file: PathBuf,
    pub qualified_name: String,
    pub start_line: u32,
    pub end_line: u32,
    pub demerits: u32,
    pub violations: Vec<IdiomViolation>,
}

struct IdiomFileVisitor {
    file: PathBuf,
    context: Vec<String>,
    struct_names: HashSet<String>,
    functions: Vec<FunctionIdioms>,
}

impl IdiomFileVisitor {
    fn new(file: PathBuf, syntax: &syn::File) -> Self {
        let mut struct_names = HashSet::new();
        for item in &syntax.items {
            if let Item::Struct(s) = item {
                struct_names.insert(s.ident.to_string());
            }
        }
        Self {
            file,
            context: Vec::new(),
            struct_names,
            functions: Vec::new(),
        }
    }

    fn record_function(
        &mut self,
        name: &str,
        sig: &syn::Signature,
        block: &syn::Block,
        is_free: bool,
    ) {
        let qualified = if self.context.is_empty() {
            name.to_string()
        } else {
            format!("{}::{name}", self.context.last().unwrap())
        };

        let start_line = sig.ident.span().start().line as u32;
        let end_line = block.brace_token.span.close().end().line as u32;

        let mut violations = Vec::new();

        if is_free && let Some(v) = check_free_method_candidate(sig, &self.struct_names, start_line)
        {
            violations.push(v);
        }

        if let Some(v) = check_box_dyn_error(&sig.output, start_line) {
            violations.push(v);
        }

        let mut body_checker = IdiomBodyChecker {
            violations: Vec::new(),
        };
        body_checker.visit_block(block);
        violations.extend(body_checker.violations);

        let demerits: u32 = violations.iter().map(|v| v.check.weight()).sum();

        self.functions.push(FunctionIdioms {
            file: self.file.clone(),
            qualified_name: qualified,
            start_line,
            end_line,
            demerits,
            violations,
        });
    }
}

impl<'ast> Visit<'ast> for IdiomFileVisitor {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if has_test_attr(&node.attrs) {
            return;
        }
        let name = node.sig.ident.to_string();
        self.record_function(&name, &node.sig, &node.block, true);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let ctx = if let Some((_, path, _)) = &node.trait_ {
            crate::complexity::format_path(path)
        } else {
            crate::complexity::format_type(&node.self_ty)
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
        self.record_function(&name, &node.sig, &node.block, false);
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
            self.record_function(&name, &node.sig, block, false);
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

// --- Body checker (per-function) ---

struct IdiomBodyChecker {
    violations: Vec<IdiomViolation>,
}

impl<'ast> Visit<'ast> for IdiomBodyChecker {
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        let literal_arms = node
            .arms
            .iter()
            .filter(|arm| is_literal_pattern(&arm.pat))
            .count();
        if literal_arms >= 2 {
            self.violations.push(IdiomViolation {
                check: IdiomCheck::MatchOnLiteral,
                line: node.match_token.span.start().line as u32,
            });
        }
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if is_comparison_or_arithmetic(&node.op)
            && (is_numeric_cast(&node.left) || is_numeric_cast(&node.right))
        {
            self.violations.push(IdiomViolation {
                check: IdiomCheck::PrimitiveCastInComparison,
                line: node.left.span().start().line as u32,
            });
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "unwrap" {
            self.violations.push(IdiomViolation {
                check: IdiomCheck::Unwrap,
                line: node.method.span().start().line as u32,
            });
        }
        if node.method == "from_iter" {
            self.violations.push(IdiomViolation {
                check: IdiomCheck::FromIterInsteadOfCollect,
                line: node.method.span().start().line as u32,
            });
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if is_call_to_name(&node.func, "drop") {
            self.violations.push(IdiomViolation {
                check: IdiomCheck::ExplicitDrop,
                line: node.func.span().start().line as u32,
            });
        }
        if is_call_to_name(&node.func, "from_iter") {
            self.violations.push(IdiomViolation {
                check: IdiomCheck::FromIterInsteadOfCollect,
                line: node.func.span().start().line as u32,
            });
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_macro(&mut self, node: &'ast syn::ExprMacro) {
        if node.mac.path.is_ident("vec") && node.mac.tokens.is_empty() {
            self.violations.push(IdiomViolation {
                check: IdiomCheck::EmptyVecMacro,
                line: node.mac.path.span().start().line as u32,
            });
        }
        syn::visit::visit_expr_macro(self, node);
    }

    fn visit_expr_closure(&mut self, _node: &'ast syn::ExprClosure) {}
}

// --- Check helpers ---

fn check_free_method_candidate(
    sig: &syn::Signature,
    struct_names: &HashSet<String>,
    line: u32,
) -> Option<IdiomViolation> {
    let first_arg = sig.inputs.first()?;
    let FnArg::Typed(pat_type) = first_arg else {
        return None;
    };

    let type_name = extract_ref_type_name(&pat_type.ty)?;
    if struct_names.contains(&type_name) {
        Some(IdiomViolation {
            check: IdiomCheck::FreeMethodCandidate,
            line,
        })
    } else {
        None
    }
}

fn extract_ref_type_name(ty: &Type) -> Option<String> {
    if let Type::Reference(r) = ty
        && let Type::Path(tp) = r.elem.as_ref()
    {
        return tp.path.segments.last().map(|s| s.ident.to_string());
    }
    None
}

fn check_box_dyn_error(output: &ReturnType, line: u32) -> Option<IdiomViolation> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };
    if contains_box_dyn_error(ty) {
        Some(IdiomViolation {
            check: IdiomCheck::BoxDynError,
            line,
        })
    } else {
        None
    }
}

fn contains_box_dyn_error(ty: &Type) -> bool {
    match ty {
        Type::Path(tp) => {
            let seg = tp.path.segments.last();
            if let Some(seg) = seg {
                if seg.ident == "Box"
                    && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
                {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(Type::TraitObject(obj)) = arg {
                            for bound in &obj.bounds {
                                if let syn::TypeParamBound::Trait(t) = bound
                                    && let Some(last) = t.path.segments.last()
                                    && last.ident == "Error"
                                {
                                    return true;
                                }
                            }
                        }
                    }
                }
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(inner) = arg
                            && contains_box_dyn_error(inner)
                        {
                            return true;
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

fn is_literal_pattern(pat: &Pat) -> bool {
    match pat {
        Pat::Lit(lit) => matches!(
            lit.lit,
            syn::Lit::Int(_) | syn::Lit::Str(_) | syn::Lit::Char(_)
        ),
        Pat::Or(or) => or.cases.iter().any(is_literal_pattern),
        _ => false,
    }
}

fn is_comparison_or_arithmetic(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::Lt(_)
            | BinOp::Le(_)
            | BinOp::Gt(_)
            | BinOp::Ge(_)
            | BinOp::Eq(_)
            | BinOp::Ne(_)
            | BinOp::Add(_)
            | BinOp::Sub(_)
            | BinOp::Mul(_)
            | BinOp::Div(_)
            | BinOp::Rem(_)
    )
}

fn is_numeric_cast(expr: &Expr) -> bool {
    if let Expr::Cast(cast) = expr {
        return is_numeric_primitive(&cast.ty);
    }
    false
}

const NUMERIC_PRIMITIVES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
    "f64",
];

fn is_numeric_primitive(ty: &Type) -> bool {
    if let Type::Path(tp) = ty
        && let Some(seg) = tp.path.segments.last()
    {
        return NUMERIC_PRIMITIVES.contains(&seg.ident.to_string().as_str());
    }
    false
}

fn is_call_to_name(func: &Expr, name: &str) -> bool {
    if let Expr::Path(ep) = func
        && let Some(seg) = ep.path.segments.last()
    {
        return seg.ident == name;
    }
    false
}

pub fn analyze_idioms_for_file(file: &std::path::Path, syntax: &syn::File) -> Vec<FunctionIdioms> {
    let mut visitor = IdiomFileVisitor::new(file.to_path_buf(), syntax);
    visitor.visit_file(syntax);
    visitor.functions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idioms(source: &str) -> Vec<(String, u32, Vec<IdiomCheck>)> {
        let syntax = syn::parse_file(source).expect("test source must parse");
        let results = analyze_idioms_for_file(std::path::Path::new("test.rs"), &syntax);
        results
            .into_iter()
            .map(|f| {
                let checks: Vec<_> = f.violations.iter().map(|v| v.check).collect();
                (f.qualified_name, f.demerits, checks)
            })
            .collect()
    }

    fn checks_for(source: &str) -> Vec<IdiomCheck> {
        let r = idioms(source);
        assert_eq!(r.len(), 1, "expected 1 function, got {}", r.len());
        r[0].2.clone()
    }

    // --- Check 1: Free method candidate ---

    #[test]
    fn free_fn_with_struct_ref_param() {
        let c = checks_for("struct Foo; fn do_thing(f: &Foo) {}");
        assert!(c.contains(&IdiomCheck::FreeMethodCandidate));
    }

    #[test]
    fn free_fn_with_mut_struct_ref_param() {
        let c = checks_for("struct Bar; fn do_thing(b: &mut Bar) {}");
        assert!(c.contains(&IdiomCheck::FreeMethodCandidate));
    }

    #[test]
    fn free_fn_with_primitive_param_is_clean() {
        let c = checks_for("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert!(!c.contains(&IdiomCheck::FreeMethodCandidate));
    }

    #[test]
    fn method_not_flagged() {
        let c = checks_for("struct S; impl S { fn method(&self) {} }");
        assert!(!c.contains(&IdiomCheck::FreeMethodCandidate));
    }

    #[test]
    fn free_fn_with_unknown_struct_not_flagged() {
        let c = checks_for("fn process(f: &SomeExternalType) {}");
        assert!(!c.contains(&IdiomCheck::FreeMethodCandidate));
    }

    // --- Check 2: Match on literals ---

    #[test]
    fn match_on_ints() {
        let c = checks_for("fn f(x: i32) { match x { 1 => {}, 2 => {}, _ => {} } }");
        assert!(c.contains(&IdiomCheck::MatchOnLiteral));
    }

    #[test]
    fn match_on_strings() {
        let c = checks_for("fn f(x: &str) { match x { \"a\" => {}, \"b\" => {}, _ => {} } }");
        assert!(c.contains(&IdiomCheck::MatchOnLiteral));
    }

    #[test]
    fn match_on_single_literal_not_flagged() {
        let c = checks_for("fn f(x: i32) { match x { 1 => {}, _ => {} } }");
        assert!(!c.contains(&IdiomCheck::MatchOnLiteral));
    }

    #[test]
    fn match_on_enum_variants_clean() {
        let c = checks_for("enum E { A, B } fn f(e: E) { match e { E::A => {}, E::B => {} } }");
        assert!(!c.contains(&IdiomCheck::MatchOnLiteral));
    }

    // --- Check 3: Primitive cast in comparison ---

    #[test]
    fn cast_in_comparison() {
        let c = checks_for("fn f(x: u32, y: u64) -> bool { x as u64 > y }");
        assert!(c.contains(&IdiomCheck::PrimitiveCastInComparison));
    }

    #[test]
    fn cast_in_arithmetic() {
        let c = checks_for("fn f(x: u32) -> u64 { x as u64 + 1 }");
        assert!(c.contains(&IdiomCheck::PrimitiveCastInComparison));
    }

    #[test]
    fn cast_for_indexing_not_flagged() {
        let c = checks_for("fn f(v: &[u8], i: u32) -> u8 { v[i as usize] }");
        assert!(!c.contains(&IdiomCheck::PrimitiveCastInComparison));
    }

    // --- Check 4: .unwrap() ---

    #[test]
    fn unwrap_detected() {
        let c = checks_for("fn f() { let _ = Some(1).unwrap(); }");
        assert!(c.contains(&IdiomCheck::Unwrap));
    }

    #[test]
    fn expect_not_flagged() {
        let c = checks_for("fn f() { let _ = Some(1).expect(\"msg\"); }");
        assert!(!c.contains(&IdiomCheck::Unwrap));
    }

    // --- Check 5: Explicit drop() ---

    #[test]
    fn explicit_drop() {
        let c = checks_for("fn f() { let x = 1; drop(x); }");
        assert!(c.contains(&IdiomCheck::ExplicitDrop));
    }

    // --- Check 6: vec![] ---

    #[test]
    fn empty_vec_macro() {
        let c = checks_for("fn f() { let _: Vec<i32> = vec![]; }");
        assert!(c.contains(&IdiomCheck::EmptyVecMacro));
    }

    #[test]
    fn vec_with_elements_clean() {
        let c = checks_for("fn f() { let _ = vec![1, 2]; }");
        assert!(!c.contains(&IdiomCheck::EmptyVecMacro));
    }

    // --- Check 7: from_iter ---

    #[test]
    fn from_iter_method_call() {
        let c = checks_for("fn f() { let _: Vec<i32> = Vec::from_iter([1, 2].iter().copied()); }");
        assert!(c.contains(&IdiomCheck::FromIterInsteadOfCollect));
    }

    // --- Check 8: Box<dyn Error> ---

    #[test]
    fn box_dyn_error_in_return() {
        let c = checks_for("fn f() -> Result<(), Box<dyn std::error::Error>> { Ok(()) }");
        assert!(c.contains(&IdiomCheck::BoxDynError));
    }

    #[test]
    fn box_dyn_error_short_path() {
        let c = checks_for("fn f() -> Result<(), Box<dyn Error>> { Ok(()) }");
        assert!(c.contains(&IdiomCheck::BoxDynError));
    }

    #[test]
    fn concrete_error_type_clean() {
        let c = checks_for("struct MyError; fn f() -> Result<(), MyError> { Ok(()) }");
        assert!(!c.contains(&IdiomCheck::BoxDynError));
    }

    // --- Closure isolation ---

    #[test]
    fn unwrap_in_closure_not_counted() {
        let c = checks_for("fn f() { let _ = || Some(1).unwrap(); }");
        assert!(!c.contains(&IdiomCheck::Unwrap));
    }

    // --- Multiple violations in one function ---

    #[test]
    fn multiple_violations_accumulate() {
        let r = idioms("fn f() { let _ = Some(1).unwrap(); let _ = Some(2).unwrap(); drop(3); }");
        assert_eq!(r[0].1, 3); // 1+1+1
    }

    // --- Weight calculation ---

    #[test]
    fn high_weight_checks() {
        assert_eq!(IdiomCheck::FreeMethodCandidate.weight(), 2);
        assert_eq!(IdiomCheck::MatchOnLiteral.weight(), 2);
        assert_eq!(IdiomCheck::PrimitiveCastInComparison.weight(), 2);
    }

    #[test]
    fn low_weight_checks() {
        assert_eq!(IdiomCheck::Unwrap.weight(), 1);
        assert_eq!(IdiomCheck::ExplicitDrop.weight(), 1);
        assert_eq!(IdiomCheck::EmptyVecMacro.weight(), 1);
        assert_eq!(IdiomCheck::FromIterInsteadOfCollect.weight(), 1);
        assert_eq!(IdiomCheck::BoxDynError.weight(), 1);
    }

    // --- Clean function ---

    #[test]
    fn clean_function_zero_demerits() {
        let r = idioms("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert_eq!(r[0].1, 0);
        assert!(r[0].2.is_empty());
    }
}
