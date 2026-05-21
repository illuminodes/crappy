use crate::complexity::{AttrExt, format_path, format_type};

pub trait FunctionVisitor {
    fn context_mut(&mut self) -> &mut Vec<String>;
    fn on_function(&mut self, name: &str, sig: &syn::Signature, block: &syn::Block, is_free: bool);

    fn handle_item_fn(&mut self, node: &syn::ItemFn) -> bool {
        if node.attrs.should_skip() {
            return false;
        }
        let name = node.sig.ident.to_string();
        self.on_function(&name, &node.sig, &node.block, true);
        true
    }

    fn handle_impl_item_fn(&mut self, node: &syn::ImplItemFn) -> bool {
        if node.attrs.should_skip() {
            return false;
        }
        let name = node.sig.ident.to_string();
        self.on_function(&name, &node.sig, &node.block, false);
        true
    }

    fn handle_item_impl_enter(&mut self, node: &syn::ItemImpl) {
        let ctx = if let Some((_, path, _)) = &node.trait_ {
            format_path(path)
        } else {
            format_type(&node.self_ty)
        };
        self.context_mut().push(ctx);
    }

    fn handle_item_impl_exit(&mut self) {
        self.context_mut().pop();
    }

    fn handle_item_trait_enter(&mut self, node: &syn::ItemTrait) {
        self.context_mut().push(node.ident.to_string());
    }

    fn handle_item_trait_exit(&mut self) {
        self.context_mut().pop();
    }

    fn handle_trait_item_fn(&mut self, node: &syn::TraitItemFn) {
        if let Some(block) = &node.default
            && !node.attrs.should_skip()
        {
            let name = node.sig.ident.to_string();
            self.on_function(&name, &node.sig, block, false);
        }
    }

    fn should_skip_mod(node: &syn::ItemMod) -> bool {
        node.attrs.has_cfg_test()
    }

    fn qualified_name(&self, name: &str) -> String
    where
        Self: Sized,
    {
        let ctx = self.context();
        if ctx.is_empty() {
            name.to_string()
        } else {
            format!("{}::{name}", ctx.last().unwrap())
        }
    }

    fn context(&self) -> &[String];
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestVisitor {
        context: Vec<String>,
        calls: Vec<(String, bool)>,
    }

    impl TestVisitor {
        fn new() -> Self {
            Self {
                context: Vec::new(),
                calls: Vec::new(),
            }
        }
    }

    impl FunctionVisitor for TestVisitor {
        fn context_mut(&mut self) -> &mut Vec<String> {
            &mut self.context
        }

        fn context(&self) -> &[String] {
            &self.context
        }

        fn on_function(
            &mut self,
            name: &str,
            _sig: &syn::Signature,
            _block: &syn::Block,
            is_free: bool,
        ) {
            self.calls.push((self.qualified_name(name), is_free));
        }
    }

    impl<'ast> syn::visit::Visit<'ast> for TestVisitor {
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

    fn visit(source: &str) -> Vec<(String, bool)> {
        use syn::visit::Visit;
        let syntax = syn::parse_file(source).expect("parse");
        let mut v = TestVisitor::new();
        v.visit_file(&syntax);
        v.calls
    }

    #[test]
    fn free_function() {
        let calls = visit("fn foo() {}");
        assert_eq!(calls, vec![("foo".into(), true)]);
    }

    #[test]
    fn impl_method_qualified() {
        let calls = visit("struct S; impl S { fn bar(&self) {} }");
        assert_eq!(calls, vec![("S::bar".into(), false)]);
    }

    #[test]
    fn trait_impl_qualified() {
        let calls = visit("struct S; trait T { fn m(&self); } impl T for S { fn m(&self) {} }");
        assert_eq!(calls, vec![("T::m".into(), false)]);
    }

    #[test]
    fn trait_default_method() {
        let calls = visit("trait T { fn default_m(&self) {} }");
        assert_eq!(calls, vec![("T::default_m".into(), false)]);
    }

    #[test]
    fn trait_abstract_method_skipped() {
        let calls = visit("trait T { fn abstract_m(&self); } fn f() {}");
        assert_eq!(calls, vec![("f".into(), true)]);
    }

    #[test]
    fn test_function_skipped() {
        let calls = visit("#[test] fn test_something() {} fn real() {}");
        assert_eq!(calls, vec![("real".into(), true)]);
    }

    #[test]
    fn test_impl_method_skipped() {
        let calls = visit("struct S; impl S { #[test] fn test_m(&self) {} fn real(&self) {} }");
        assert_eq!(calls, vec![("S::real".into(), false)]);
    }

    #[test]
    fn cfg_test_module_skipped() {
        let calls = visit("fn visible() {} #[cfg(test)] mod tests { fn hidden() {} }");
        assert_eq!(calls, vec![("visible".into(), true)]);
    }

    #[test]
    fn crappy_allow_skips_free_fn() {
        let calls = visit("#[crappy_allow] fn skip() {} fn keep() {}");
        assert_eq!(calls, vec![("keep".into(), true)]);
    }

    #[test]
    fn crappy_allow_skips_impl_method() {
        let calls =
            visit("struct S; impl S { #[crappy_allow] fn skip(&self) {} fn keep(&self) {} }");
        assert_eq!(calls, vec![("S::keep".into(), false)]);
    }

    #[test]
    fn crappy_allow_skips_trait_default() {
        let calls = visit("trait T { #[crappy_allow] fn skip(&self) {} fn keep(&self) {} }");
        assert_eq!(calls, vec![("T::keep".into(), false)]);
    }

    #[test]
    fn context_cleaned_after_impl() {
        let calls = visit("struct A; impl A { fn m(&self) {} } fn free() {}");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "A::m");
        assert_eq!(calls[1].0, "free");
    }

    #[test]
    fn context_cleaned_after_trait() {
        let calls = visit("trait T { fn m(&self) {} } fn free() {}");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "T::m");
        assert_eq!(calls[1].0, "free");
    }
}
