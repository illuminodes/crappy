use std::collections::HashSet;
use std::path::PathBuf;

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

pub struct FunctionIdioms {
    pub file: PathBuf,
    pub qualified_name: String,
    pub start_line: u32,
    pub end_line: u32,
    pub demerits: u32,
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

        let mut checks = Vec::new();

        if is_free && is_free_method_candidate(sig, &self.struct_names) {
            checks.push(IdiomCheck::FreeMethodCandidate);
        }

        if has_box_dyn_error(&sig.output) {
            checks.push(IdiomCheck::BoxDynError);
        }

        let mut body_checker = IdiomBodyChecker { checks: Vec::new() };
        body_checker.visit_block(block);
        checks.extend(body_checker.checks);

        let demerits: u32 = checks.iter().map(|c| c.weight()).sum();

        self.functions.push(FunctionIdioms {
            file: self.file.clone(),
            qualified_name: qualified,
            start_line,
            end_line,
            demerits,
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
    checks: Vec<IdiomCheck>,
}

impl<'ast> Visit<'ast> for IdiomBodyChecker {
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        let literal_arms = node
            .arms
            .iter()
            .filter(|arm| is_literal_pattern(&arm.pat))
            .count();
        if literal_arms >= 2 {
            self.checks.push(IdiomCheck::MatchOnLiteral);
        }
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if is_comparison_or_arithmetic(&node.op)
            && (is_numeric_cast(&node.left) || is_numeric_cast(&node.right))
        {
            self.checks.push(IdiomCheck::PrimitiveCastInComparison);
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "unwrap" {
            self.checks.push(IdiomCheck::Unwrap);
        }
        if node.method == "from_iter" {
            self.checks.push(IdiomCheck::FromIterInsteadOfCollect);
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if is_call_to_name(&node.func, "drop") {
            self.checks.push(IdiomCheck::ExplicitDrop);
        }
        if is_call_to_name(&node.func, "from_iter") {
            self.checks.push(IdiomCheck::FromIterInsteadOfCollect);
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_macro(&mut self, node: &'ast syn::ExprMacro) {
        if node.mac.path.is_ident("vec") && node.mac.tokens.is_empty() {
            self.checks.push(IdiomCheck::EmptyVecMacro);
        }
        syn::visit::visit_expr_macro(self, node);
    }

    fn visit_expr_closure(&mut self, _node: &'ast syn::ExprClosure) {}
}

// --- Check helpers ---

fn is_free_method_candidate(sig: &syn::Signature, struct_names: &HashSet<String>) -> bool {
    let Some(FnArg::Typed(pat_type)) = sig.inputs.first() else {
        return false;
    };
    extract_ref_type_name(&pat_type.ty).is_some_and(|name| struct_names.contains(&name))
}

fn extract_ref_type_name(ty: &Type) -> Option<String> {
    if let Type::Reference(r) = ty
        && let Type::Path(tp) = r.elem.as_ref()
    {
        return tp.path.segments.last().map(|s| s.ident.to_string());
    }
    None
}

fn has_box_dyn_error(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };
    contains_box_dyn_error(ty)
}

fn contains_box_dyn_error(ty: &Type) -> bool {
    let Type::Path(tp) = ty else { return false };
    let Some(seg) = tp.path.segments.last() else {
        return false;
    };

    if seg.ident == "Box"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
    {
        for arg in &args.args {
            if let syn::GenericArgument::Type(Type::TraitObject(obj)) = arg
                && obj.bounds.iter().any(|b| {
                    matches!(b, syn::TypeParamBound::Trait(t)
                            if t.path.segments.last().is_some_and(|s| s.ident == "Error"))
                })
            {
                return true;
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

    false
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

    fn demerits_for(source: &str) -> u32 {
        let syntax = syn::parse_file(source).expect("test source must parse");
        let results = analyze_idioms_for_file(std::path::Path::new("test.rs"), &syntax);
        assert_eq!(
            results.len(),
            1,
            "expected 1 function, got {}",
            results.len()
        );
        results[0].demerits
    }

    fn all_demerits(source: &str) -> Vec<(String, u32)> {
        let syntax = syn::parse_file(source).expect("test source must parse");
        let results = analyze_idioms_for_file(std::path::Path::new("test.rs"), &syntax);
        results
            .into_iter()
            .map(|f| (f.qualified_name, f.demerits))
            .collect()
    }

    // --- Check 1: Free method candidate (weight 2) ---

    #[test]
    fn free_fn_with_struct_ref_param() {
        assert_eq!(demerits_for("struct Foo; fn do_thing(f: &Foo) {}"), 2);
    }

    #[test]
    fn free_fn_with_mut_struct_ref_param() {
        assert_eq!(demerits_for("struct Bar; fn do_thing(b: &mut Bar) {}"), 2);
    }

    #[test]
    fn free_fn_with_primitive_param_is_clean() {
        assert_eq!(demerits_for("fn add(a: i32, b: i32) -> i32 { a + b }"), 0);
    }

    #[test]
    fn method_not_flagged() {
        assert_eq!(demerits_for("struct S; impl S { fn method(&self) {} }"), 0);
    }

    #[test]
    fn free_fn_with_unknown_struct_not_flagged() {
        assert_eq!(demerits_for("fn process(f: &SomeExternalType) {}"), 0);
    }

    // --- Check 2: Match on literals (weight 2) ---

    #[test]
    fn match_on_ints() {
        assert_eq!(
            demerits_for("fn f(x: i32) { match x { 1 => {}, 2 => {}, _ => {} } }"),
            2
        );
    }

    #[test]
    fn match_on_strings() {
        assert_eq!(
            demerits_for("fn f(x: &str) { match x { \"a\" => {}, \"b\" => {}, _ => {} } }"),
            2
        );
    }

    #[test]
    fn match_on_single_literal_not_flagged() {
        assert_eq!(
            demerits_for("fn f(x: i32) { match x { 1 => {}, _ => {} } }"),
            0
        );
    }

    #[test]
    fn match_on_enum_variants_clean() {
        assert_eq!(
            demerits_for("enum E { A, B } fn f(e: E) { match e { E::A => {}, E::B => {} } }"),
            0
        );
    }

    // --- Check 3: Primitive cast in comparison (weight 2) ---

    #[test]
    fn cast_in_comparison() {
        assert_eq!(
            demerits_for("fn f(x: u32, y: u64) -> bool { x as u64 > y }"),
            2
        );
    }

    #[test]
    fn cast_in_arithmetic() {
        assert_eq!(demerits_for("fn f(x: u32) -> u64 { x as u64 + 1 }"), 2);
    }

    #[test]
    fn cast_for_indexing_not_flagged() {
        assert_eq!(
            demerits_for("fn f(v: &[u8], i: u32) -> u8 { v[i as usize] }"),
            0
        );
    }

    // --- Check 4: .unwrap() (weight 1) ---

    #[test]
    fn unwrap_detected() {
        assert_eq!(demerits_for("fn f() { let _ = Some(1).unwrap(); }"), 1);
    }

    #[test]
    fn expect_not_flagged() {
        assert_eq!(
            demerits_for("fn f() { let _ = Some(1).expect(\"msg\"); }"),
            0
        );
    }

    // --- Check 5: Explicit drop() (weight 1) ---

    #[test]
    fn explicit_drop() {
        assert_eq!(demerits_for("fn f() { let x = 1; drop(x); }"), 1);
    }

    // --- Check 6: vec![] (weight 1) ---

    #[test]
    fn empty_vec_macro() {
        assert_eq!(demerits_for("fn f() { let _: Vec<i32> = vec![]; }"), 1);
    }

    #[test]
    fn vec_with_elements_clean() {
        assert_eq!(demerits_for("fn f() { let _ = vec![1, 2]; }"), 0);
    }

    // --- Check 7: from_iter (weight 1) ---

    #[test]
    fn from_iter_method_call() {
        assert_eq!(
            demerits_for("fn f() { let _: Vec<i32> = Vec::from_iter([1, 2].iter().copied()); }"),
            1
        );
    }

    // --- Check 8: Box<dyn Error> (weight 1) ---

    #[test]
    fn box_dyn_error_in_return() {
        assert_eq!(
            demerits_for("fn f() -> Result<(), Box<dyn std::error::Error>> { Ok(()) }"),
            1
        );
    }

    #[test]
    fn box_dyn_error_short_path() {
        assert_eq!(
            demerits_for("fn f() -> Result<(), Box<dyn Error>> { Ok(()) }"),
            1
        );
    }

    #[test]
    fn concrete_error_type_clean() {
        assert_eq!(
            demerits_for("struct MyError; fn f() -> Result<(), MyError> { Ok(()) }"),
            0
        );
    }

    // --- Closure isolation ---

    #[test]
    fn unwrap_in_closure_not_counted() {
        assert_eq!(demerits_for("fn f() { let _ = || Some(1).unwrap(); }"), 0);
    }

    // --- Accumulation ---

    #[test]
    fn multiple_violations_accumulate() {
        // 2x unwrap (1+1) + 1x drop (1) = 3
        let r =
            all_demerits("fn f() { let _ = Some(1).unwrap(); let _ = Some(2).unwrap(); drop(3); }");
        assert_eq!(r[0].1, 3);
    }

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

    #[test]
    fn clean_function_zero_demerits() {
        assert_eq!(demerits_for("fn add(a: i32, b: i32) -> i32 { a + b }"), 0);
    }
}
