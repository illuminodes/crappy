use crate::complexity::{format_path, format_type, has_cfg_test, has_test_attr};

pub trait FunctionVisitor {
    fn context_mut(&mut self) -> &mut Vec<String>;
    fn on_function(&mut self, name: &str, sig: &syn::Signature, block: &syn::Block, is_free: bool);

    fn handle_item_fn(&mut self, node: &syn::ItemFn) -> bool {
        if has_test_attr(&node.attrs) {
            return false;
        }
        let name = node.sig.ident.to_string();
        self.on_function(&name, &node.sig, &node.block, true);
        true
    }

    fn handle_impl_item_fn(&mut self, node: &syn::ImplItemFn) -> bool {
        if has_test_attr(&node.attrs) {
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
            && !has_test_attr(&node.attrs)
        {
            let name = node.sig.ident.to_string();
            self.on_function(&name, &node.sig, block, false);
        }
    }

    fn should_skip_mod(node: &syn::ItemMod) -> bool {
        has_cfg_test(&node.attrs)
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
