// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Rules of the workspace's Rust the compiler does not check, read from
//! each file's syntax tree:
//!
//! - `silent_drops`: a module that carries [`MARKER`] records every failure
//!   it ignores with `vnilink::trace::ignored!`, so no result is dropped
//!   with `let _ = …;` or `….ok();` outside its test modules.
//! - `engine_spans`: every public `&self` method of `impl Core` starts in
//!   the engine's span (`let _engine = self.engine();`), so what it records
//!   says which engine recorded it.
//! - `test_style`: every test of the kit's suites returns a `Result` and
//!   starts by holding `traced()`, so its setup fails with `?` and a
//!   failure prints what the engines recorded.
//!
//! Each check has tests on literal source beside the one that reads the
//! workspace.

use std::fs;
use std::path::{Path, PathBuf};

use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Attribute, Block, Expr, ImplItem, Item, Pat, Stmt, Token};
use vnilink_testkit::TestResult;

/// The line a module carries once every failure it ignores is recorded
/// with `vnilink::trace::ignored!`.
const MARKER: &str = "// trace: records every ignored failure";

/// The `Core` methods that manage the engine's span itself.
const SPAN_OWNERS: [&str; 2] = ["trace_span", "set_trace_name"];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.rs` file under `folder`, build output left out, in a stable order.
fn rust_files(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let path = entry?.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "target") {
                    folders.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn relative(path: &Path) -> String {
    let root = workspace();
    path.strip_prefix(&root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Whether `attrs` hold a `#[cfg]` that only a test build satisfies:
/// `test` itself, or `all(…)` with `test` among its terms.
fn test_only(attrs: &[Attribute]) -> bool {
    fn requires_test(meta: &syn::Meta) -> bool {
        match meta {
            syn::Meta::Path(path) => path.is_ident("test"),
            syn::Meta::List(list) if list.path.is_ident("all") => list
                .parse_args_with(Punctuated::<syn::Meta, Token![,]>::parse_terminated)
                .is_ok_and(|terms| terms.iter().any(requires_test)),
            _ => false,
        }
    }
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Meta>()
                .is_ok_and(|meta| requires_test(&meta))
    })
}

/// The name a call goes by: a method's name, or a function path's last
/// segment. Used to say which call a finding is about.
fn called(expr: &Expr) -> String {
    match expr {
        Expr::MethodCall(call) => call.method.to_string(),
        Expr::Call(call) => called(&call.func),
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default(),
        Expr::Await(awaited) => format!("{}.await", called(&awaited.base)),
        Expr::Macro(mac) => mac
            .mac
            .path
            .segments
            .last()
            .map(|segment| format!("{}!", segment.ident))
            .unwrap_or_default(),
        Expr::Paren(inner) => called(&inner.expr),
        _ => "an expression".to_owned(),
    }
}

/// A name, or a tuple of names: `let _ = name;` only marks names unused.
fn names_only(expr: &Expr) -> bool {
    match expr {
        Expr::Path(path) => path.qself.is_none() && path.path.get_ident().is_some(),
        Expr::Paren(inner) => names_only(&inner.expr),
        Expr::Tuple(tuple) => !tuple.elems.is_empty() && tuple.elems.iter().all(names_only),
        _ => false,
    }
}

/// Results a module drops without recording them, outside its test modules:
/// `let _ = expr;`, unless `expr` is a name, a tuple of names, or passes the
/// failure on with `?`; and a statement `expr.ok();`. Each finding names the
/// function it is in and the call whose result is dropped.
fn silent_drops(file: &syn::File) -> Vec<String> {
    #[derive(Default)]
    struct Drops {
        functions: Vec<String>,
        found: Vec<String>,
    }

    impl Drops {
        fn report(&mut self, how: &str, expr: &Expr) {
            let function = self.functions.last().map_or_else(
                || "outside a function".to_owned(),
                |name| format!("fn {name}"),
            );
            self.found
                .push(format!("{function}: {how} drops `{}`", called(expr)));
        }

        /// A macro's input is tokens to the parser. Reads it as statements,
        /// then as comma-separated expressions, and otherwise looks inside
        /// each braced group, where `select!` and its kin keep their arms.
        fn visit_tokens(&mut self, tokens: proc_macro2::TokenStream) {
            if let Ok(stmts) = Block::parse_within.parse2(tokens.clone()) {
                for stmt in &stmts {
                    self.visit_stmt(stmt);
                }
                return;
            }
            if let Ok(exprs) =
                Punctuated::<Expr, Token![,]>::parse_terminated.parse2(tokens.clone())
            {
                for expr in &exprs {
                    self.visit_expr(expr);
                }
                return;
            }
            for tree in tokens {
                if let proc_macro2::TokenTree::Group(group) = tree {
                    self.visit_tokens(group.stream());
                }
            }
        }
    }

    impl<'ast> Visit<'ast> for Drops {
        fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
            if !test_only(&module.attrs) {
                visit::visit_item_mod(self, module);
            }
        }

        fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
            self.functions.push(function.sig.ident.to_string());
            visit::visit_item_fn(self, function);
            self.functions.pop();
        }

        fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
            self.functions.push(function.sig.ident.to_string());
            visit::visit_impl_item_fn(self, function);
            self.functions.pop();
        }

        fn visit_trait_item_fn(&mut self, function: &'ast syn::TraitItemFn) {
            self.functions.push(function.sig.ident.to_string());
            visit::visit_trait_item_fn(self, function);
            self.functions.pop();
        }

        fn visit_local(&mut self, local: &'ast syn::Local) {
            if let (Pat::Wild(_), Some(init)) = (&local.pat, &local.init) {
                if !names_only(&init.expr) && !matches!(*init.expr, Expr::Try(_)) {
                    self.report("`let _ =`", &init.expr);
                }
            }
            visit::visit_local(self, local);
        }

        fn visit_stmt(&mut self, stmt: &'ast Stmt) {
            if let Stmt::Expr(Expr::MethodCall(call), Some(_)) = stmt {
                if call.method == "ok" && call.args.is_empty() {
                    self.report("`.ok();`", &call.receiver);
                }
            }
            visit::visit_stmt(self, stmt);
        }

        fn visit_macro(&mut self, mac: &'ast syn::Macro) {
            self.visit_tokens(mac.tokens.clone());
        }
    }

    let mut drops = Drops::default();
    drops.visit_file(file);
    drops.found
}

/// The public `&self` methods of `impl Core` blocks, outside test modules,
/// whose body does not start in the engine's span
/// (`let _engine = self.engine();`).
fn unnamed_methods(file: &syn::File) -> Vec<String> {
    fn starts_in_span(block: &Block) -> bool {
        let Some(Stmt::Local(local)) = block.stmts.first() else {
            return false;
        };
        let Pat::Ident(name) = &local.pat else {
            return false;
        };
        let Some(init) = &local.init else {
            return false;
        };
        let Expr::MethodCall(call) = &*init.expr else {
            return false;
        };
        name.ident == "_engine"
            && name.by_ref.is_none()
            && name.mutability.is_none()
            && name.subpat.is_none()
            && init.diverge.is_none()
            && call.method == "engine"
            && call.args.is_empty()
            && call.turbofish.is_none()
            && matches!(&*call.receiver, Expr::Path(path) if path.path.is_ident("self"))
    }

    fn visit_items(items: &[Item], found: &mut Vec<String>) {
        for item in items {
            match item {
                Item::Mod(module) if !test_only(&module.attrs) => {
                    if let Some((_, items)) = &module.content {
                        visit_items(items, found);
                    }
                }
                Item::Impl(block) if block.trait_.is_none() => {
                    let syn::Type::Path(ty) = &*block.self_ty else {
                        continue;
                    };
                    if ty.qself.is_some() || !ty.path.is_ident("Core") {
                        continue;
                    }
                    for item in &block.items {
                        let ImplItem::Fn(method) = item else {
                            continue;
                        };
                        let shared_self = matches!(
                            method.sig.receiver(),
                            Some(receiver)
                                if receiver.colon_token.is_none()
                                    && receiver.reference.is_some()
                                    && receiver.mutability.is_none()
                        );
                        let name = method.sig.ident.to_string();
                        if matches!(method.vis, syn::Visibility::Public(_))
                            && shared_self
                            && !SPAN_OWNERS.contains(&name.as_str())
                            && !starts_in_span(&method.block)
                        {
                            found.push(name);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut found = Vec::new();
    visit_items(&file.items, &mut found);
    found
}

/// The tests in `file` that do not return a `Result` or do not start by
/// holding `traced()`, each with what it lacks.
fn unstyled_tests(file: &syn::File) -> Vec<String> {
    fn is_test(attrs: &[Attribute]) -> bool {
        attrs.iter().any(|attr| attr.path().is_ident("test"))
    }

    fn returns_result(output: &syn::ReturnType) -> bool {
        let syn::ReturnType::Type(_, ty) = output else {
            return false;
        };
        let syn::Type::Path(ty) = &**ty else {
            return false;
        };
        ty.path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "TestResult" || segment.ident == "Result")
    }

    /// The first statement that runs, after the `use`s, constants and
    /// types a test declares, is `let <name> = traced();`.
    fn holds_trace_first(block: &Block) -> bool {
        let first = block
            .stmts
            .iter()
            .find(|stmt| !matches!(stmt, Stmt::Item(_)));
        let Some(Stmt::Local(local)) = first else {
            return false;
        };
        let Some(init) = &local.init else {
            return false;
        };
        let Expr::Call(call) = &*init.expr else {
            return false;
        };
        let Expr::Path(function) = &*call.func else {
            return false;
        };
        matches!(local.pat, Pat::Ident(_))
            && call.args.is_empty()
            && function
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "traced")
    }

    fn visit_items(items: &[Item], found: &mut Vec<String>) {
        for item in items {
            match item {
                Item::Mod(module) => {
                    if let Some((_, items)) = &module.content {
                        visit_items(items, found);
                    }
                }
                Item::Fn(function) if is_test(&function.attrs) => {
                    let name = function.sig.ident.to_string();
                    if !returns_result(&function.sig.output) {
                        found.push(format!("fn {name}: returns no Result"));
                    }
                    if !holds_trace_first(&function.block) {
                        found.push(format!("fn {name}: does not start by holding traced()"));
                    }
                }
                _ => {}
            }
        }
    }

    let mut found = Vec::new();
    visit_items(&file.items, &mut found);
    found
}

fn parse(source: &str) -> syn::File {
    syn::parse_file(source).expect("the snippet is Rust")
}

#[test]
fn a_dropped_result_is_found_and_names_and_passed_failures_are_not() {
    let file = parse(
        r#"
        fn drops() {
            // let _ = in_a_comment();
            let _ = sender.send(1);
            value.ok();
            keep(value.ok());
            let _ = unused;
            let _ = (a, b);
            let _ = (a);
            let kept = value.ok();
            let _ = passes()?;
            list
                .pop()
                .ok();
            if ready { let _ = nested.close(); }
            let close = || { let _ = in_closure(); };
        }
        "#,
    );
    assert_eq!(
        silent_drops(&file),
        [
            "fn drops: `let _ =` drops `send`",
            "fn drops: `.ok();` drops `value`",
            "fn drops: `.ok();` drops `pop`",
            "fn drops: `let _ =` drops `close`",
            "fn drops: `let _ =` drops `in_closure`",
        ],
        "a call's result dropped with `let _ =` or `.ok();` is found; names, `?` and kept values are not"
    );
}

#[test]
fn a_dropped_result_inside_a_macro_is_found() {
    let file = parse(
        r#"
        async fn selects() {
            tokio::select! {
                _ = stop.cancelled() => { let _ = reply.send(()); }
                value = queue.recv() => { value.ok(); }
            }
            vec![{ let _ = in_a_list(); 1 }];
        }
        "#,
    );
    assert_eq!(
        silent_drops(&file),
        [
            "fn selects: `let _ =` drops `send`",
            "fn selects: `.ok();` drops `value`",
            "fn selects: `let _ =` drops `in_a_list`",
        ],
        "a macro's input is read as the code it holds"
    );
}

#[test]
fn a_recorded_failure_and_test_modules_are_not_silent_drops() {
    let file = parse(
        r#"
        fn records() {
            vnilink::ignored!("vnilink::trace::files", "the watcher is gone", watcher.stop());
            let value = vnilink::trace::ignored!("vnilink::trace::files", "the size is unknown", file.len());
        }
        #[cfg(test)]
        mod tests {
            fn fixture() { let _ = setup(); }
        }
        #[cfg(all(test, loom))]
        mod loom_model {
            fn model() { let _ = handle.join(); }
        }
        #[cfg(not(test))]
        mod shipped {
            fn shipped() { let _ = still_found(); }
        }
        "#,
    );
    assert_eq!(
        silent_drops(&file),
        ["fn shipped: `let _ =` drops `still_found`"],
        "`ignored!` records the failure, and only modules a test build alone compiles are skipped"
    );
}

#[test]
fn modules_that_record_every_ignored_failure_drop_nothing_silently() -> TestResult {
    let mut marked = 0;
    let mut offending = Vec::new();
    for path in rust_files(&workspace().join("crates"))? {
        let source = fs::read_to_string(&path)?;
        if !source.contains(MARKER) {
            continue;
        }
        marked += 1;
        let file =
            syn::parse_file(&source).map_err(|error| format!("{}: {error}", relative(&path)))?;
        for drop in silent_drops(&file) {
            offending.push(format!("{}: {drop}", relative(&path)));
        }
    }
    assert!(marked > 0, "no module carries \"{MARKER}\"");
    assert!(
        offending.is_empty(),
        "record these with vnilink::ignored! and a reason:\n{}",
        offending.join("\n")
    );
    Ok(())
}

#[test]
fn a_core_method_outside_the_engine_span_is_found_and_others_are_not() {
    let file = parse(
        r#"
        impl Core {
            pub fn named(&self) -> u8 {
                let _engine = self.engine();
                1
            }
            pub fn unnamed(
                &self,
                value: u8,
            ) -> u8 {
                value
            }
            pub fn late(&self) {
                let value = 1;
                let _engine = self.engine();
            }
            pub fn other_span(&self) {
                let _engine = self.other();
            }
            pub fn set_trace_name(&self, name: &str) {
                self.shared.span.record("name", name);
            }
            pub(crate) fn internal(&self) {}
            pub fn exclusive(&mut self) {}
            pub fn constructor() -> Self { todo!() }
        }
        impl Drop for Core {
            fn drop(&mut self) {}
        }
        impl Other {
            pub fn elsewhere(&self) {}
        }
        mod nested {
            impl Core {
                pub fn inner(&self) {}
            }
        }
        #[cfg(test)]
        mod tests {
            impl Core {
                pub fn probe(&self) {}
            }
        }
        "#,
    );
    assert_eq!(
        unnamed_methods(&file),
        ["unnamed", "late", "other_span", "inner"],
        "only public `&self` methods of `impl Core` must start in the engine's span"
    );
}

#[test]
fn every_public_core_method_records_under_its_engine() -> TestResult {
    let mut offending = Vec::new();
    for path in rust_files(&workspace().join("crates/vnilink/src"))? {
        let source = fs::read_to_string(&path)?;
        let file =
            syn::parse_file(&source).map_err(|error| format!("{}: {error}", relative(&path)))?;
        for method in unnamed_methods(&file) {
            offending.push(format!("{}: {method}", relative(&path)));
        }
    }
    assert!(
        offending.is_empty(),
        "start these with `let _engine = self.engine();`:\n{}",
        offending.join("\n")
    );
    Ok(())
}

#[test]
fn a_test_without_a_result_or_a_trace_first_is_found() {
    let file = parse(
        r#"
        #[test]
        fn styled() -> TestResult {
            let trace = traced();
            Ok(())
        }
        #[test]
        fn quiet() -> Result<(), Error> {
            let _trace = vnilink_testkit::traced();
            Ok(())
        }
        #[test]
        fn declares_first() -> TestResult {
            use std::time::Duration;
            const WAIT: Duration = Duration::from_secs(1);
            struct Fake;
            let _trace = traced();
            Ok(())
        }
        #[test]
        fn returns_nothing() {
            let trace = traced();
        }
        #[test]
        fn traced_late() -> TestResult {
            let pair = Pair::new()?;
            let trace = traced();
            Ok(())
        }
        #[test]
        fn neither() {}
        fn helper() {}
        mod nested {
            #[test]
            fn inner() {}
        }
        "#,
    );
    assert_eq!(
        unstyled_tests(&file),
        [
            "fn returns_nothing: returns no Result",
            "fn traced_late: does not start by holding traced()",
            "fn neither: returns no Result",
            "fn neither: does not start by holding traced()",
            "fn inner: returns no Result",
            "fn inner: does not start by holding traced()",
        ],
        "a test returns a Result and holds traced() first; helpers are not tests"
    );
}

/// The kit's suites, but this one: it reads source and starts no engine.
#[test]
fn every_kit_test_returns_a_result_and_holds_its_trace() -> TestResult {
    let tests = workspace().join("crates/vnilink-testkit/tests");
    let mut offending = Vec::new();
    for path in rust_files(&tests)? {
        if path == tests.join("tidy.rs") {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        let file =
            syn::parse_file(&source).map_err(|error| format!("{}: {error}", relative(&path)))?;
        for test in unstyled_tests(&file) {
            offending.push(format!("{}: {test}", relative(&path)));
        }
    }
    assert!(
        offending.is_empty(),
        "return TestResult and start with `let trace = traced();`:\n{}",
        offending.join("\n")
    );
    Ok(())
}
