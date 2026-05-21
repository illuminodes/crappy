use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use syn::visit::Visit;
use syn::{BinOp, Expr, FnArg, Item, Pat, ReturnType, Type};

use crate::visitor::FunctionVisitor;

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
    pub const fn weight(self) -> u32 {
        match self {
            Self::FreeMethodCandidate | Self::MatchOnLiteral | Self::PrimitiveCastInComparison => 2,
            _ => 1,
        }
    }

    pub const fn suggestion(self) -> &'static str {
        match self {
            Self::FreeMethodCandidate => {
                "first parameter is &Struct — consider making this a method"
            }
            Self::MatchOnLiteral => "match on literal values — consider using an enum instead",
            Self::PrimitiveCastInComparison => {
                "primitive `as` cast in comparison/arithmetic — use From/Into traits"
            }
            Self::Unwrap => ".unwrap() call — use `?` or `.expect(\"reason\")`",
            Self::ExplicitDrop => "explicit drop() call — use a scoped block instead",
            Self::EmptyVecMacro => "vec![] for empty vector — use Vec::new()",
            Self::FromIterInsteadOfCollect => "FromIterator::from_iter() — use .collect() instead",
            Self::BoxDynError => "Box<dyn Error> in return type — use a concrete error type",
        }
    }
}

pub struct FunctionIdioms {
    pub file: PathBuf,
    pub qualified_name: String,
    pub demerits: u32,
    pub checks: Vec<IdiomCheck>,
    pub sig_duplicate: bool,
    pub body_duplicate: bool,
    pub sig_fingerprint: String,
    pub body_fingerprint: String,
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
}

impl FunctionVisitor for IdiomFileVisitor {
    fn context_mut(&mut self) -> &mut Vec<String> {
        &mut self.context
    }

    fn context(&self) -> &[String] {
        &self.context
    }

    fn on_function(&mut self, name: &str, sig: &syn::Signature, block: &syn::Block, is_free: bool) {
        let qualified = self.qualified_name(name);

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
        let sig_fingerprint = fingerprint_sig(sig, self.context.last());
        let body_fingerprint = fingerprint_body(block);

        self.functions.push(FunctionIdioms {
            file: self.file.clone(),
            qualified_name: qualified,
            demerits,
            checks,
            sig_duplicate: false,
            body_duplicate: false,
            sig_fingerprint,
            body_fingerprint,
        });
    }
}

impl<'ast> Visit<'ast> for IdiomFileVisitor {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if self.handle_item_fn(node) {
            syn::visit::visit_item_fn(self, node);
        }
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.handle_item_impl_enter(node);
        syn::visit::visit_item_impl(self, node);
        self.handle_item_impl_exit();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if self.handle_impl_item_fn(node) {
            syn::visit::visit_impl_item_fn(self, node);
        }
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.handle_item_trait_enter(node);
        syn::visit::visit_item_trait(self, node);
        self.handle_item_trait_exit();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.handle_trait_item_fn(node);
        syn::visit::visit_trait_item_fn(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !Self::should_skip_mod(node) {
            syn::visit::visit_item_mod(self, node);
        }
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

const fn is_comparison_or_arithmetic(op: &BinOp) -> bool {
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

// --- Fingerprinting for dryness checks ---

fn fingerprint_sig(sig: &syn::Signature, self_type: Option<&String>) -> String {
    let mut parts = Vec::new();
    for input in &sig.inputs {
        match input {
            FnArg::Receiver(r) => {
                let ty = self_type.map_or("Self", String::as_str);
                let mutability = if r.mutability.is_some() {
                    format!("&mut {ty}")
                } else {
                    format!("&{ty}")
                };
                parts.push(mutability);
            }
            FnArg::Typed(t) => parts.push(type_fingerprint(&t.ty)),
        }
    }
    let ret = match &sig.output {
        ReturnType::Default => "()".to_string(),
        ReturnType::Type(_, ty) => type_fingerprint(ty),
    };
    format!("({})->{ret}", parts.join(","))
}

fn type_fingerprint(ty: &Type) -> String {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map_or_else(|| "_".to_string(), leak_ident),
        Type::Reference(r) => {
            let mutability = if r.mutability.is_some() { "&mut " } else { "&" };
            format!("{mutability}{}", type_fingerprint(&r.elem))
        }
        Type::Tuple(t) => {
            let inner: Vec<_> = t.elems.iter().map(type_fingerprint).collect();
            format!("({})", inner.join(","))
        }
        Type::Slice(s) => format!("[{}]", type_fingerprint(&s.elem)),
        Type::ImplTrait(_) => "impl_".to_string(),
        _ => "_".to_string(),
    }
}

fn leak_ident(seg: &syn::PathSegment) -> String {
    let base = seg.ident.to_string();
    if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
        let generics: Vec<_> = args
            .args
            .iter()
            .filter_map(|a| {
                if let syn::GenericArgument::Type(t) = a {
                    Some(type_fingerprint(t))
                } else {
                    None
                }
            })
            .collect();
        if generics.is_empty() {
            base
        } else {
            format!("{base}<{}>", generics.join(","))
        }
    } else {
        base
    }
}

fn fingerprint_body(block: &syn::Block) -> String {
    let mut hasher = std::hash::DefaultHasher::new();
    let mut normalizer = BodyNormalizer { tokens: Vec::new() };
    normalizer.visit_block(block);
    for token in &normalizer.tokens {
        token.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

struct BodyNormalizer {
    tokens: Vec<&'static str>,
}

impl<'ast> Visit<'ast> for BodyNormalizer {
    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        self.tokens.push("if");
        syn::visit::visit_expr_if(self, node);
    }
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        self.tokens.push("match");
        syn::visit::visit_expr_match(self, node);
    }
    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        self.tokens.push("arm");
        syn::visit::visit_arm(self, node);
    }
    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.tokens.push("while");
        syn::visit::visit_expr_while(self, node);
    }
    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.tokens.push("for");
        syn::visit::visit_expr_for_loop(self, node);
    }
    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.tokens.push("loop");
        syn::visit::visit_expr_loop(self, node);
    }
    fn visit_expr_return(&mut self, node: &'ast syn::ExprReturn) {
        self.tokens.push("return");
        syn::visit::visit_expr_return(self, node);
    }
    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        self.tokens.push("?");
        syn::visit::visit_expr_try(self, node);
    }
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        self.tokens.push("call");
        syn::visit::visit_expr_call(self, node);
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.tokens.push("method");
        syn::visit::visit_expr_method_call(self, node);
    }
    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        self.tokens.push("binop");
        syn::visit::visit_expr_binary(self, node);
    }
    fn visit_expr_unary(&mut self, node: &'ast syn::ExprUnary) {
        self.tokens.push("unop");
        syn::visit::visit_expr_unary(self, node);
    }
    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        self.tokens.push("assign");
        syn::visit::visit_expr_assign(self, node);
    }
    fn visit_expr_closure(&mut self, _node: &'ast syn::ExprClosure) {
        self.tokens.push("closure");
    }
    fn visit_local(&mut self, node: &'ast syn::Local) {
        self.tokens.push("let");
        syn::visit::visit_local(self, node);
    }
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

    // --- Trait context ---

    #[test]
    fn trait_default_method_checked() {
        let r = all_demerits("trait T { fn default_impl(&self) { let _ = Some(1).unwrap(); } }");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "T::default_impl");
        assert_eq!(r[0].1, 1);
    }

    #[test]
    fn trait_method_without_body_skipped() {
        let r = all_demerits("trait T { fn abstract_method(&self); } fn f() {}");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "f");
    }

    // --- cfg(test) module skipping ---

    #[test]
    fn cfg_test_module_skipped() {
        let r = all_demerits(
            "fn visible() { let _ = Some(1).unwrap(); }
             #[cfg(test)] mod tests { fn hidden() { let _ = Some(1).unwrap(); } }",
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "visible");
    }

    // --- Match on char literals ---

    #[test]
    fn match_on_chars() {
        assert_eq!(
            demerits_for("fn f(c: char) { match c { 'a' => {}, 'b' => {}, _ => {} } }"),
            2
        );
    }

    // --- Match with Or patterns ---

    #[test]
    fn match_or_pattern_with_literals() {
        assert_eq!(
            demerits_for("fn f(x: i32) { match x { 1 | 2 => {}, 3 | 4 => {}, _ => {} } }"),
            2
        );
    }

    // --- from_iter as function call ---

    #[test]
    fn from_iter_function_call() {
        assert_eq!(
            demerits_for(
                "fn f() { let _: Vec<i32> = std::iter::FromIterator::from_iter(vec![1]); }"
            ),
            1
        );
    }

    // --- Box<dyn Error + Send + Sync> ---

    #[test]
    fn box_dyn_error_with_send_sync() {
        assert_eq!(
            demerits_for("fn f() -> Result<(), Box<dyn Error + Send + Sync>> { Ok(()) }"),
            1
        );
    }

    // --- Fingerprint tests ---

    #[test]
    fn sig_fingerprint_includes_impl_context() {
        let syntax = syn::parse_file(
            "struct A; struct B; impl A { fn m(&self) {} } impl B { fn m(&self) {} }",
        )
        .unwrap();
        let results = analyze_idioms_for_file(std::path::Path::new("test.rs"), &syntax);
        assert_eq!(results.len(), 2);
        assert_ne!(results[0].sig_fingerprint, results[1].sig_fingerprint);
    }

    #[test]
    fn body_fingerprint_same_structure() {
        let syntax =
            syn::parse_file("fn a(x: bool) { if x { } } fn b(y: bool) { if y { } }").unwrap();
        let results = analyze_idioms_for_file(std::path::Path::new("test.rs"), &syntax);
        assert_eq!(results[0].body_fingerprint, results[1].body_fingerprint);
    }

    #[test]
    fn body_fingerprint_different_structure() {
        let syntax =
            syn::parse_file("fn a(x: bool) { if x { } } fn b() { loop { break; } }").unwrap();
        let results = analyze_idioms_for_file(std::path::Path::new("test.rs"), &syntax);
        assert_ne!(results[0].body_fingerprint, results[1].body_fingerprint);
    }
}
