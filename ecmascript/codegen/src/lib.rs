#![feature(box_syntax)]
#![feature(box_patterns)]
#![feature(specialization)]
#![feature(proc_macro_hygiene)]
#![feature(trace_macros)]
#![recursion_limit = "1024"]
#![allow(unused_variables)]

#[macro_use]
extern crate bitflags;
extern crate sourcemap;
extern crate swc_atoms;
extern crate swc_ecma_codegen_macros;
#[macro_use]
extern crate swc_common;
extern crate swc_ecma_ast;

pub use self::config::{Config, SourceMapConfig};
use self::{
    list::ListFormat,
    text_writer::WriteJs,
    util::{SourceMapperExt, SpanExt, StartsWithAlphaNum},
};
use std::{collections::HashSet, io};
use swc_atoms::JsWord;
use swc_common::{sync::Lrc, BytePos, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_ecma_ast::*;
use swc_ecma_codegen_macros::emitter;

#[macro_use]
pub mod macros;
mod comments;
mod config;
mod decl;
pub mod list;
#[cfg(test)]
mod tests;
pub mod text_writer;
pub mod util;

pub type Result = io::Result<()>;

pub trait Handlers {
    // fn on_before_emit_token(&mut self, _node: &Any) {}
    // fn on_after_emit_token(&mut self, _node: &Any) {}
}

pub trait Node: Spanned {
    fn emit_with(&self, e: &mut Emitter) -> Result;
}
impl<N: Node> Node for Box<N> {
    fn emit_with(&self, e: &mut Emitter) -> Result {
        (**self).emit_with(e)
    }
}
impl<'a, N: Node> Node for &'a N {
    fn emit_with(&self, e: &mut Emitter) -> Result {
        (**self).emit_with(e)
    }
}

pub struct Emitter<'a> {
    pub cfg: config::Config,
    pub cm: Lrc<SourceMap>,
    pub wr: Box<('a + WriteJs)>,
    pub handlers: Box<('a + Handlers)>,
    pub pos_of_leading_comments: HashSet<BytePos>,
}

impl<'a> Emitter<'a> {
    pub fn emit_script(&mut self, stmts: &[Stmt]) -> Result {
        let span = if stmts.is_empty() {
            DUMMY_SP
        } else {
            stmts[0].span().with_hi(
                stmts
                    .last()
                    .map(|s| s.span().hi())
                    .unwrap_or(stmts[0].span().lo()),
            )
        };

        self.emit_list(span, Some(stmts), ListFormat::SourceFileStatements)?;

        Ok(())
    }

    #[emitter]
    pub fn emit_module(&mut self, node: &Module) -> Result {
        for stmt in &node.body {
            emit!(stmt);
        }
    }

    #[emitter]
    pub fn emit_module_item(&mut self, node: &ModuleItem) -> Result {
        match *node {
            ModuleItem::Stmt(ref stmt) => emit!(stmt),
            ModuleItem::ModuleDecl(ref decl) => emit!(decl),
        }
    }

    #[emitter]
    pub fn emit_module_decl(&mut self, node: &ModuleDecl) -> Result {
        match *node {
            ModuleDecl::Import(ref d) => emit!(d),
            ModuleDecl::ExportDecl(ref d) => {
                keyword!("export");
                space!();
                emit!(d);
                semi!();
            }
            ModuleDecl::ExportNamed(ref d) => emit!(d),
            ModuleDecl::ExportDefaultDecl(ref d) => emit!(d),
            ModuleDecl::ExportDefaultExpr(ref d) => {
                keyword!("export");
                space!();
                keyword!("default");
                space!();
                emit!(d);
                semi!();
            }
            ModuleDecl::ExportAll(ref d) => emit!(d),
        }
    }

    #[emitter]
    pub fn emit_export_default_decl(&mut self, node: &ExportDefaultDecl) -> Result {
        keyword!("export");
        space!();
        keyword!("default");
        space!();
        match *node {
            ExportDefaultDecl::Class(ref class) => emit!(class),
            ExportDefaultDecl::Fn(ref f) => emit!(f),
            ExportDefaultDecl::Var(ref decl) => emit!(decl),
        }
        semi!();
    }

    #[emitter]
    pub fn emit_import(&mut self, node: &ImportDecl) -> Result {
        keyword!("import");
        space!();

        let mut specifiers = vec![];
        for specifier in &node.specifiers {
            match specifier {
                ImportSpecifier::Specific(ref s) => {
                    specifiers.push(s);
                }
                ImportSpecifier::Default(ref s) => {
                    emit!(s.local);
                    space!();
                }
                ImportSpecifier::Namespace(ref ns) => {
                    assert!(node.specifiers.len() <= 2);
                    punct!("*");
                    space!();
                    keyword!("as");
                    space!();
                    emit!(ns.local);
                    space!();
                }
            }
        }

        if !specifiers.is_empty() {
            punct!("{");
            self.emit_list(
                node.span(),
                Some(&specifiers),
                ListFormat::NamedImportsOrExportsElements,
            )?;
            punct!("}");
        }

        keyword!("from");
        space!();
        emit!(node.src);
        semi!();
    }

    #[emitter]
    pub fn emit_import_specific(&mut self, node: &ImportSpecific) -> Result {
        if let Some(ref imported) = node.imported {
            emit!(imported);
            space!();
            keyword!("as");
            space!();
        }

        emit!(node.local);
    }

    #[emitter]
    pub fn emit_export_specifier(&mut self, node: &ExportSpecifier) -> Result {
        if let Some(ref exported) = node.exported {
            emit!(node.orig);
            space!();
            keyword!("as");
            space!();
            emit!(node.exported);
        } else {
            emit!(node.orig);
        }
    }

    #[emitter]
    pub fn emit_named_export(&mut self, node: &NamedExport) -> Result {
        keyword!("export");
        formatting_space!();
        punct!("{");
        self.emit_list(
            node.span,
            Some(&node.specifiers),
            ListFormat::NamedImportsOrExportsElements,
        )?;
        // TODO:
        punct!("}");
        if let Some(ref src) = node.src {
            space!();
            keyword!("from");
            emit!(src);
            semi!();
        }
    }

    #[emitter]
    pub fn emit_export_all(&mut self, node: &ExportAll) -> Result {
        keyword!("export");
        space!();
        punct!("*");
        formatting_space!();
        keyword!("from");
        space!();
        emit!(node.src);
        semi!();
    }

    #[emitter]
    pub fn emit_lit(&mut self, node: &Lit) -> Result {
        match *node {
            Lit::Bool(Bool { value, span }) => {
                if value {
                    keyword!(span, "true")
                } else {
                    keyword!(span, "false")
                }
            }
            Lit::Null(Null { span }) => keyword!(span, "null"),
            Lit::Str(ref s) => emit!(s),
            Lit::Num(ref n) => emit!(n),
            Lit::Regex(ref n) => {
                punct!("/");
                self.emit_js_word(n.exp.span, &n.exp.value)?;
                punct!("/");
                if let Some(ref flags) = n.flags {
                    self.emit_js_word(flags.span, &flags.value)?;
                }
            }
        }
    }

    fn emit_js_word(&mut self, span: Span, value: &JsWord) -> Result {
        self.wr.write_str_lit(span, &value)?;

        Ok(())
    }

    #[emitter]
    pub fn emit_str_lit(&mut self, node: &Str) -> Result {
        // TODO: quote
        if let Some(s) = get_text_of_node(&self.cm, node, false) {
            self.wr.write_str_lit(node.span, &s)?;
        } else {
            punct!("'");
            self.wr.write_str_lit(node.span, &node.value)?;
            punct!("'");
        }
    }

    #[emitter]
    pub fn emit_num_lit(&mut self, num: &Number) -> Result {
        // FIXME: Emitter might de-optimize

        match get_text_of_node(&self.cm, num, false) {
            Some(s) => self.wr.write_str_lit(num.span, &s)?,
            None => {
                // Handle infinity
                if num.value.is_infinite() {
                    if num.value.is_sign_negative() {
                        self.wr.write_str_lit(num.span, "-")?;
                    }
                    self.wr.write_str_lit(num.span, "Infinity")?;
                } else {
                    self.wr.write_str_lit(num.span, &format!("{}", num.value))?
                }
            }
        };

        return Ok(());
    }

    // pub fn emit_object_binding_pat(&mut self, node: &ObjectPat) -> Result {
    //     self.wr.write_punct("{")?;
    //     self.emit_list(
    //         node.span(),
    //         &node.props,
    //         ListFormat::ObjectBindingPatternElements,
    //     );
    //     self.wr.write_punct("}")?;

    //     Ok(())
    // }

    // pub fn emit_array_binding_pat(&mut self, node: &ArrayPat) -> Result {
    //     self.wr.write_punct("[")?;
    //     self.emit_list(
    //         node.span(),
    //         &node.elems,
    //         ListFormat::ArrayBindingPatternElements,
    //     );
    //     self.wr.write_punct("]")?;

    //     Ok(())
    // }

    #[emitter]
    pub fn emit_expr_or_super(&mut self, node: &ExprOrSuper) -> Result {
        match *node {
            ExprOrSuper::Expr(ref e) => emit!(e),
            ExprOrSuper::Super(_) => keyword!("super"),
        }
    }

    #[emitter]
    pub fn emit_expr(&mut self, node: &Expr) -> Result {
        match *node {
            Expr::Array(ref n) => emit!(n),
            Expr::Arrow(ref n) => emit!(n),
            Expr::Assign(ref n) => emit!(n),
            Expr::Await(ref n) => emit!(n),
            Expr::Bin(ref n) => emit!(n),
            Expr::Call(ref n) => emit!(n),
            Expr::Class(ref n) => emit!(n),
            Expr::Cond(ref n) => emit!(n),
            Expr::Fn(ref n) => emit!(n),
            Expr::Ident(ref n) => emit!(n),
            Expr::Lit(ref n) => emit!(n),
            Expr::Member(ref n) => emit!(n),
            Expr::MetaProp(ref n) => emit!(n),
            Expr::New(ref n) => emit!(n),
            Expr::Object(ref n) => emit!(n),
            Expr::Paren(ref n) => emit!(n),
            Expr::Seq(ref n) => emit!(n),
            Expr::This(ref n) => emit!(n),
            Expr::Tpl(ref n) => emit!(n),
            Expr::Unary(ref n) => emit!(n),
            Expr::Update(ref n) => emit!(n),
            Expr::Yield(ref n) => emit!(n),
        }
    }

    #[emitter]
    pub fn emit_call_expr(&mut self, node: &CallExpr) -> Result {
        emit!(node.callee);
        punct!("(");
        self.emit_expr_or_spreads(node.span(), &node.args, ListFormat::CallExpressionArguments)?;
        punct!(")");
    }

    #[emitter]
    pub fn emit_new_expr(&mut self, node: &NewExpr) -> Result {
        keyword!("new");

        space!();
        emit!(node.callee);

        if let Some(ref args) = node.args {
            punct!("(");
            self.emit_expr_or_spreads(node.span(), args, ListFormat::NewExpressionArguments)?;
            punct!(")");
        }
    }

    #[emitter]
    pub fn emit_member_expr(&mut self, node: &MemberExpr) -> Result {
        emit!(node.obj);

        if node.computed {
            punct!("[");
            emit!(node.prop);
            punct!("]");
        } else {
            if self.needs_2dots_for_property_access(&node.obj) {
                punct!(".");
            }
            punct!(".");
            emit!(node.prop);
        }
    }

    /// `1..toString` is a valid property access, emit a dot after the literal
    pub fn needs_2dots_for_property_access(&self, expr: &ExprOrSuper) -> bool {
        match *expr {
            ExprOrSuper::Expr(box Expr::Lit(Lit::Num(Number { span, .. }))) => {
                // check if numeric literal is a decimal literal that was originally written
                // with a dot
                let text = self.cm.span_to_string(span);
                return !text.contains(".");
            }
            _ => false,
        }
    }

    #[emitter]
    pub fn emit_arrow_expr(&mut self, node: &ArrowExpr) -> Result {
        if node.async_token.is_some() {
            keyword!("async");
            formatting_space!();
        }
        if node.generator_token.is_some() {
            punct!("*")
        }
        punct!("(");
        self.emit_list(node.span, Some(&node.params), ListFormat::CommaListElements)?;
        punct!(")");

        punct!("=>");
        emit!(node.body);
    }

    #[emitter]
    pub fn emit_meta_prop_expr(&mut self, node: &MetaPropExpr) -> Result {
        emit!(node.meta);
        punct!(".");
        emit!(node.prop);
    }

    #[emitter]
    pub fn emit_seq_expr(&mut self, node: &SeqExpr) -> Result {
        let mut first = true;
        //TODO: Indention
        for e in &node.exprs {
            if first {
                first = false
            } else {
                punct!(",");
            }
            formatting_space!();

            emit!(e);
        }
    }

    #[emitter]
    pub fn emit_assign_expr(&mut self, node: &AssignExpr) -> Result {
        emit!(node.left);
        formatting_space!();
        operator!(node.op.as_str());
        formatting_space!();
        emit!(node.right);
    }

    #[emitter]
    pub fn emit_bin_expr(&mut self, node: &BinExpr) -> Result {
        // let indent_before_op = needs_indention(node, &node.left, node.op);
        // let indent_after_op = needs_indention(node, node.op, &node.right);

        emit!(node.left);
        formatting_space!();
        operator!(node.op.as_str());
        formatting_space!();
        emit!(node.right);
    }

    #[emitter]
    pub fn emit_class_expr(&mut self, node: &ClassExpr) -> Result {
        keyword!("class");

        opt_leading_space!(node.ident);

        self.emit_class_trailing(&node.class)?;
    }

    #[emitter]
    pub fn emit_class_trailing(&mut self, node: &Class) -> Result {
        if node.super_class.is_some() {
            space!();
            keyword!("extends");
            space!();
            emit!(node.super_class);
        }

        punct!("{");
        self.emit_list(node.span, Some(&node.body), ListFormat::ClassMembers)?;
        punct!("}");
    }

    #[emitter]
    pub fn emit_class_method(&mut self, node: &ClassMethod) -> Result {
        if let Some(_st) = node.static_token {
            keyword!("static");
            space!();
        }
        match node.kind {
            ClassMethodKind::Constructor => {
                keyword!("constructor");
            }
            ClassMethodKind::Method => {
                if let Some(async) = node.function.async_token {
                    keyword!("async");
                }
                space!();
                if let Some(generator) = node.function.generator_token {
                    punct!("*");
                }

                emit!(node.key);
            }
            ClassMethodKind::Getter => {
                keyword!("get");
                space!();

                emit!(node.key);
            }
            ClassMethodKind::Setter => {
                keyword!("set");
                space!();

                emit!(node.key);
            }
        }

        self.emit_fn_trailing(&node.function)?;
    }

    #[emitter]
    pub fn emit_prop_name(&mut self, node: &PropName) -> Result {
        match *node {
            PropName::Ident(ref n) => emit!(n),
            PropName::Str(ref n) => emit!(n),
            PropName::Num(ref n) => emit!(n),
            PropName::Computed(ref n) => emit!(n),
        }
    }

    #[emitter]
    pub fn emit_cond_expr(&mut self, node: &CondExpr) -> Result {
        // TODO: Indent

        emit!(node.test);
        formatting_space!();
        punct!("?");
        formatting_space!();
        emit!(node.cons);
        formatting_space!();
        punct!(":");
        formatting_space!();
        emit!(node.alt);
    }

    #[emitter]
    pub fn emit_fn_expr(&mut self, node: &FnExpr) -> Result {
        if node.function.async_token.is_some() {
            keyword!("async");
        }
        keyword!("function");

        if node.function.generator_token.is_some() {
            punct!("*");
        }
        opt_leading_space!(node.ident);

        self.emit_fn_trailing(&node.function)?;
    }

    #[emitter]
    /// prints `(b){}` from `function a(b){}`
    pub fn emit_fn_trailing(&mut self, node: &Function) -> Result {
        punct!("(");
        self.emit_list(node.span, Some(&node.params), ListFormat::CommaListElements)?;
        punct!(")");

        formatting_space!();
        emit!(node.body);
    }

    #[emitter]
    pub fn emit_block_stmt_or_expr(&mut self, node: &BlockStmtOrExpr) -> Result {
        match *node {
            BlockStmtOrExpr::BlockStmt(ref block_stmt) => emit!(block_stmt),
            BlockStmtOrExpr::Expr(ref expr) => {
                self.wr.increase_indent()?;
                emit!(expr);
                self.wr.decrease_indent()?;
            }
        }
    }

    #[emitter]
    pub fn emit_this_expr(&mut self, node: &ThisExpr) -> Result {
        keyword!("this");
    }

    #[emitter]
    pub fn emit_tpl_lit(&mut self, node: &TplLit) -> Result {
        debug_assert!(node.quasis.len() == node.exprs.len() + 1);

        if let Some(ref tag) = node.tag {
            emit!(tag);
        }
        punct!("`");
        let i = 0;
        println!("{:?}", node.exprs.len() + node.quasis.len());

        for i in 0..(node.quasis.len() + node.exprs.len()) {
            if i % 2 == 0 {
                emit!(node.quasis[i / 2]);
            } else {
                punct!("$");
                punct!("{");
                emit!(node.exprs[i / 2]);
                punct!("}");
            }
        }

        punct!("`");
    }

    #[emitter]
    pub fn emit_quasi(&mut self, node: &TplElement) -> Result {
        self.wr.write_str_lit(node.span, &node.raw)?;
        return Ok(());
    }

    #[emitter]
    pub fn emit_unary_expr(&mut self, node: &UnaryExpr) -> Result {
        //TODO: Operator vs Keyword
        keyword!(node.op.as_str());
        if should_emit_whitespace_before_operand(node) {
            space!();
        } else {
            // TODO:
            // formatting_space!();
        }

        emit!(node.arg);
    }

    #[emitter]
    pub fn emit_update_expr(&mut self, node: &UpdateExpr) -> Result {
        if node.prefix {
            operator!(node.op.as_str());
            //TODO: Check if we should use should_emit_whitespace_before_operand
            emit!(node.arg);
        } else {
            emit!(node.arg);
            operator!(node.op.as_str());
        }
    }

    #[emitter]
    pub fn emit_yield_expr(&mut self, node: &YieldExpr) -> Result {
        keyword!("yield");
        if node.delegate {
            operator!("*");
        }
        opt_leading_space!(node.arg);
    }

    pub fn emit_expr_or_spreads(
        &mut self,
        parent_node: Span,
        nodes: &[ExprOrSpread],
        format: ListFormat,
    ) -> Result {
        self.emit_list(parent_node, Some(nodes), format)
    }

    #[emitter]
    pub fn emit_expr_or_spread(&mut self, node: &ExprOrSpread) -> Result {
        if node.spread.is_some() {
            punct!("...");
        }

        emit!(node.expr);
    }

    #[emitter]
    pub fn emit_await_expr(&mut self, node: &AwaitExpr) -> Result {
        // TODO: Comment
        keyword!("await");

        space!();

        emit!(&node.arg);
    }

    #[emitter]
    pub fn emit_array_lit(&mut self, node: &ArrayLit) -> Result {
        punct!("[");
        self.emit_list(
            node.span(),
            Some(&node.elems),
            ListFormat::ArrayLiteralExpressionElements,
        )?;
        punct!("]");
    }

    #[emitter]
    pub fn emit_object_lit(&mut self, node: &ObjectLit) -> Result {
        // const preferNewLine = node.multiLine ? ListFormat.PreferNewLine :
        // ListFormat.None;

        // const allowTrailingComma =
        // currentSourceFile.languageVersion >= ScriptTarget.ES5 ?
        // ListFormat.AllowTrailingComma : ListFormat.None; emitList(node,
        // node.properties, ListFormat.ObjectLiteralExpressionProperties |
        // allowTrailingComma | preferNewLine);

        punct!("{");
        self.wr.write_line()?;
        self.wr.increase_indent()?;
        self.emit_list(
            node.span(),
            Some(&node.props),
            ListFormat::ObjectLiteralExpressionProperties,
        )?;
        self.wr.write_line()?;
        self.wr.decrease_indent()?;
        punct!("}");
    }

    #[emitter]
    pub fn emit_prop(&mut self, node: &Prop) -> Result {
        match *node {
            Prop::Shorthand(ref n) => emit!(n),
            Prop::KeyValue(ref n) => emit!(n),
            Prop::Assign(ref n) => emit!(n),
            Prop::Getter(ref n) => emit!(n),
            Prop::Setter(ref n) => emit!(n),
            Prop::Method(ref n) => emit!(n),
        }
    }

    #[emitter]
    pub fn emit_kv_prop(&mut self, node: &KeyValueProp) -> Result {
        emit!(node.key);
        punct!(":");
        formatting_space!();
        emit!(node.value);
    }

    #[emitter]
    pub fn emit_assign_prop(&mut self, node: &AssignProp) -> Result {
        emit!(node.key);
        punct!("=");
        emit!(node.value);
    }

    #[emitter]
    pub fn emit_getter_prop(&mut self, node: &GetterProp) -> Result {
        keyword!("get");
        space!();
        emit!(node.key);
        space!();
        punct!("(");
        punct!(")");
        formatting_space!();
        emit!(node.body);
    }

    #[emitter]
    pub fn emit_setter_prop(&mut self, node: &SetterProp) -> Result {
        keyword!("set");
        space!();
        emit!(node.key);
        space!();

        punct!("(");
        emit!(node.param);
        punct!(")");

        emit!(node.body);
    }

    #[emitter]
    pub fn emit_method_prop(&mut self, node: &MethodProp) -> Result {
        if let Some(_gen) = node.function.generator_token {
            punct!("*");
        }

        emit!(node.key);
        space!();
        // TODO
        self.emit_fn_trailing(&node.function)?;
    }

    #[emitter]
    pub fn emit_paren_expr(&mut self, node: &ParenExpr) -> Result {
        punct!("(");
        emit!(node.expr);
        punct!(")");
    }

    #[emitter]
    pub fn emit_ident(&mut self, ident: &Ident) -> Result {
        // TODO: Use write_symbol when ident is a symbol.
        self.emit_leading_comments_of_pos(ident.span.lo())?;

        let symbol: Option<String> = None;
        if let Some(sym) = symbol {
            //            self.wr.write_symbol(
            //     &get_text_of_node(, &ident, /* includeTrivia */ false),
            //     sym,
            // )?;
            unimplemented!()
        } else {
            // TODO: span
            if let Some(s) = get_text_of_node(&self.cm, &ident, false) {
                self.wr.write_symbol(ident.span, &s)?
            } else {
                self.wr.write_symbol(ident.span, &ident.sym)?
            }

            // self.wr
            //     .write(get_text_of_node(&self.cm, &ident, /* includeTrivia */
            // false).as_bytes())?;
        }

        //TODO

        // Call emitList directly since it could be an array of
        // TypeParameterDeclarations _or_ type arguments

        // emitList(node, node.typeArguments, ListFormat::TypeParameters);
    }

    pub fn emit_list<N: Node>(
        &mut self,
        parent_node: Span,
        children: Option<&[N]>,
        format: ListFormat,
    ) -> Result {
        self.emit_list5(
            parent_node,
            children,
            format,
            0,
            children.map(|c| c.len()).unwrap_or(0),
        )
    }

    pub fn emit_list5<N: Node>(
        &mut self,
        parent_node: Span,
        children: Option<&[N]>,
        format: ListFormat,
        start: usize,
        count: usize,
    ) -> Result {
        if children.is_none() && format.contains(ListFormat::OptionalIfUndefined) {
            return Ok(());
        }

        let is_empty = children.is_none() || start > children.unwrap().len() || count == 0;
        if is_empty && format.contains(ListFormat::OptionalIfEmpty) {
            // self.handlers.onBeforeEmitNodeArray(children)

            // self.handlers.onAfterEmitNodeArray(children);

            return Ok(());
        }

        if format.contains(ListFormat::BracketsMask) {
            self.wr.write_punct(format.opening_bracket())?;

            if is_empty {
                self.emit_trailing_comments_of_pos(
                    {
                        // TODO: children.lo()

                        parent_node.lo()
                    },
                    true,
                )?;
            }
        }

        // self.handlers.onBeforeEmitNodeArray(children);

        if is_empty {
            // Write a line terminator if the parent node was multi-line

            if format.contains(ListFormat::MultiLine) {
                self.wr.write_line()?;
            } else if format.contains(ListFormat::SpaceBetweenBraces)
                && !(format.contains(ListFormat::NoSpaceIfEmpty))
            {
                self.wr.write_space()?;
            }
        } else {
            let children = children.unwrap();

            // Write the opening line terminator or leading whitespace.
            let may_emit_intervening_comments =
                !format.intersects(ListFormat::NoInterveningComments);
            let mut should_emit_intervening_comments = may_emit_intervening_comments;
            if self
                .cm
                .should_write_leading_line_terminator(parent_node, children, format)
            {
                self.wr.write_line()?;
                should_emit_intervening_comments = false;
            } else if format.contains(ListFormat::SpaceBetweenBraces) {
                self.wr.write_space()?;
            }

            // Increase the indent, if requested.
            if format.contains(ListFormat::Indented) {
                self.wr.increase_indent()?;
            }

            // Emit each child.
            let mut previous_sibling: Option<Span> = None;
            let mut should_decrease_indent_after_emit = false;
            for i in 0..count {
                let child = &children[start + i];

                // Write the delimiter if this is not the first node.
                if let Some(previous_sibling) = previous_sibling {
                    // i.e
                    //      function commentedParameters(
                    //          /* Parameter a */
                    //          a
                    // /* End of parameter a */
                    // -> this comment isn't considered to be trailing comment of parameter "a" due
                    // to newline ,
                    if format.contains(ListFormat::DelimitersMask)
                        && previous_sibling.span().hi() != parent_node.hi()
                    {
                        self.emit_leading_comments_of_pos(previous_sibling.span().hi())?;
                    }
                    self.write_delim(format)?;

                    // Write either a line terminator or whitespace to separate the elements.

                    if self.cm.should_write_separating_line_terminator(
                        Some(previous_sibling),
                        Some(child),
                        format,
                    ) {
                        // If a synthesized node in a single-line list starts on a new
                        // line, we should increase the indent.
                        if (format & (ListFormat::LinesMask | ListFormat::Indented))
                            == ListFormat::SingleLine
                        {
                            self.wr.increase_indent()?;
                            should_decrease_indent_after_emit = true;
                        }

                        self.wr.write_line()?;
                        should_emit_intervening_comments = false;
                    } else if format.contains(ListFormat::SpaceBetweenSiblings) {
                        self.wr.write_space()?;
                    }
                }

                // Emit this child.
                if should_emit_intervening_comments {
                    let comment_range = child.comment_range();
                    self.emit_trailing_comments_of_pos(comment_range.hi(), false)?;
                } else {
                    should_emit_intervening_comments = may_emit_intervening_comments;
                }

                child.emit_with(self)?;

                if should_decrease_indent_after_emit {
                    self.wr.decrease_indent()?;
                    should_decrease_indent_after_emit = false;
                }

                previous_sibling = Some(child.span());
            }

            // Write a trailing comma, if requested.
            let has_trailing_comma = format.contains(ListFormat::AllowTrailingComma) && {
                // children.hasTrailingComma
                false
            };
            if format.contains(ListFormat::CommaDelimited) && has_trailing_comma {
                self.wr.write_punct(",")?;
            }

            {
                // Emit any trailing comment of the last element in the list
                // i.e
                //       var array = [...
                //          2
                //          /* end of element 2 */
                //       ];

                let emit_trailing_comments = {
                    // TODO:
                    //
                    // !(getEmitFlags(previousSibling).contains(EmitFlags::NoTrailingComments))

                    true
                };

                if let Some(previous_sibling) = previous_sibling {
                    if format.contains(ListFormat::DelimitersMask)
                        && previous_sibling.span().hi() != parent_node.hi()
                        && emit_trailing_comments
                    {
                        self.emit_leading_comments_of_pos(previous_sibling.span().hi())?;
                    }
                }
            }

            // Decrease the indent, if requested.
            if format.contains(ListFormat::Indented) {
                self.wr.decrease_indent()?;
            }

            // Write the closing line terminator or closing whitespace.
            if self
                .cm
                .should_write_closing_line_terminator(parent_node, children, format)
            {
                self.wr.write_line()?;
            } else if format.contains(ListFormat::SpaceBetweenBraces) {
                self.wr.write_space()?;
            }
        }

        // self.handlers.onAfterEmitNodeArray(children);

        if format.contains(ListFormat::BracketsMask) {
            if is_empty {
                self.emit_leading_comments_of_pos({
                    //TODO: children.hi()

                    parent_node.hi()
                })?; // Emit leading comments within empty lists
            }
            self.wr.write_punct(format.closing_bracket())?;
        }

        Ok(())
    }
}

/// Patterns
impl<'a> Emitter<'a> {
    #[emitter]
    pub fn emit_pat(&mut self, node: &Pat) -> Result {
        match *node {
            Pat::Array(ref n) => emit!(n),
            Pat::Assign(ref n) => emit!(n),
            Pat::Expr(ref n) => emit!(n),
            Pat::Ident(ref n) => emit!(n),
            Pat::Object(ref n) => emit!(n),
            Pat::Rest(ref n) => emit!(n),
        }
    }

    #[emitter]
    pub fn emit_rest_pat(&mut self, node: &RestPat) -> Result {
        punct!("...");
        emit!(node.arg);
    }

    #[emitter]
    pub fn emit_prop_or_spread(&mut self, node: &PropOrSpread) -> Result {
        match *node {
            PropOrSpread::Prop(ref n) => emit!(n),
            PropOrSpread::Spread(ref n) => emit!(n),
        }
    }

    #[emitter]
    pub fn emit_spread_element(&mut self, node: &SpreadElement) -> Result {
        punct!("...");
        emit!(node.expr)
    }

    #[emitter]
    pub fn emit_pat_or_expr(&mut self, node: &PatOrExpr) -> Result {
        match *node {
            PatOrExpr::Expr(ref n) => emit!(n),
            PatOrExpr::Pat(ref n) => emit!(n),
        }
    }

    #[emitter]
    pub fn emit_array_pat(&mut self, node: &ArrayPat) -> Result {
        punct!("[");
        self.emit_list(
            node.span(),
            Some(&node.elems),
            ListFormat::ArrayBindingPatternElements,
        )?;
        punct!("]");
    }

    #[emitter]
    pub fn emit_assign_pat(&mut self, node: &AssignPat) -> Result {
        emit!(node.left);
        formatting_space!();
        punct!("=");
        formatting_space!();
        emit!(node.right);
    }

    #[emitter]
    pub fn emit_object_pat(&mut self, node: &ObjectPat) -> Result {
        punct!("{");
        self.emit_list(
            node.span(),
            Some(&node.props),
            ListFormat::ObjectBindingPatternElements,
        )?;
        punct!("}");
    }

    #[emitter]
    pub fn emit_object_pat_prop(&mut self, node: &ObjectPatProp) -> Result {
        match *node {
            ObjectPatProp::KeyValue(ref node) => emit!(node),
            ObjectPatProp::Assign(ref node) => emit!(node),
            ObjectPatProp::Rest(ref node) => emit!(node),
        }
    }

    #[emitter]
    pub fn emit_object_kv_pat(&mut self, node: &KeyValuePatProp) -> Result {
        emit!(node.key);
        punct!(":");
        formatting_space!();
        emit!(node.value);
        space!();
    }

    #[emitter]
    pub fn emit_object_assign_pat(&mut self, node: &AssignPatProp) -> Result {
        emit!(node.key);
        space!();
        if let Some(ref value) = node.value {
            punct!("=");
            emit!(node.value);
            space!();
        }
    }

    #[emitter]
    pub fn emit_var_decl_or_pat(&mut self, node: &VarDeclOrPat) -> Result {
        match *node {
            VarDeclOrPat::Pat(ref n) => emit!(n),
            VarDeclOrPat::VarDecl(ref n) => emit!(n),
        }
    }
}

/// Statements
impl<'a> Emitter<'a> {
    pub fn emit_stmts(&mut self, stmts: &[Stmt]) -> Result {
        for stmt in stmts {
            self.emit_stmt(stmt)?;
        }

        Ok(())
    }

    #[emitter]
    pub fn emit_stmt(&mut self, node: &Stmt) -> Result {
        match *node {
            Stmt::Expr(ref e) => {
                emit!(e);
                semi!();
            }
            Stmt::Block(ref e) => {
                emit!(e);
                return Ok(());
            }
            Stmt::Empty(ref e) => emit!(e),
            Stmt::Debugger(ref e) => emit!(e),
            Stmt::With(ref e) => emit!(e),
            Stmt::Return(ref e) => emit!(e),
            Stmt::Labeled(ref e) => emit!(e),
            Stmt::Break(ref e) => emit!(e),
            Stmt::Continue(ref e) => emit!(e),
            Stmt::If(ref e) => emit!(e),
            Stmt::Switch(ref e) => emit!(e),
            Stmt::Throw(ref e) => emit!(e),
            Stmt::Try(ref e) => emit!(e),
            Stmt::While(ref e) => emit!(e),
            Stmt::DoWhile(ref e) => emit!(e),
            Stmt::For(ref e) => emit!(e),
            Stmt::ForIn(ref e) => emit!(e),
            Stmt::ForOf(ref e) => emit!(e),
            Stmt::Decl(ref e) => emit!(e),
        }
        self.wr.write_line()?;
    }

    #[emitter]
    pub fn emit_block_stmt(&mut self, node: &BlockStmt) -> Result {
        punct!("{");
        self.emit_list(
            node.span(),
            Some(&node.stmts),
            ListFormat::MultiLineBlockStatements,
        )?;
        punct!("}");
    }

    #[emitter]
    pub fn emit_empty_stmt(&mut self, node: &EmptyStmt) -> Result {
        punct!(";")
    }

    #[emitter]
    pub fn emit_debugger_stmt(&mut self, node: &DebuggerStmt) -> Result {
        keyword!("debugger");
        semi!();
    }

    #[emitter]
    pub fn emit_with_stmt(&mut self, node: &WithStmt) -> Result {
        keyword!("with");
        formatting_space!();

        punct!("(");
        emit!(node.obj);
        punct!(")");

        emit!(node.body);
    }

    #[emitter]
    pub fn emit_return_stmt(&mut self, node: &ReturnStmt) -> Result {
        keyword!("return");
        if let Some(ref arg) = node.arg {
            space!();
            emit!(arg)
        }
        semi!();
    }

    #[emitter]
    pub fn emit_labeled_stmt(&mut self, node: &LabeledStmt) -> Result {
        emit!(node.label);

        // TODO: Comment
        punct!(":");
        space!();

        emit!(node.body);
    }

    #[emitter]
    pub fn emit_break_stmt(&mut self, node: &BreakStmt) -> Result {
        keyword!("break");
        opt_leading_space!(node.label);
        semi!();
    }

    #[emitter]
    pub fn emit_continue_stmt(&mut self, node: &ContinueStmt) -> Result {
        keyword!("continue");
        opt_leading_space!(node.label);
        semi!();
    }

    #[emitter]
    pub fn emit_if_stmt(&mut self, node: &IfStmt) -> Result {
        keyword!("if");

        space!();
        punct!("(");
        emit!(node.test);
        punct!(")");
        space!();

        let is_block_stmt = match *node.cons {
            Stmt::Block(_) => true,
            _ => false,
        };
        emit!(node.cons);

        if let Some(ref alt) = node.alt {
            if is_block_stmt {
                space!();
            }
            keyword!("else");
            space!();
            emit!(alt);
        }
    }

    #[emitter]
    pub fn emit_switch_stmt(&mut self, node: &SwitchStmt) -> Result {
        keyword!("switch");

        punct!("(");
        emit!(node.discriminant);
        punct!(")");

        punct!("{");
        self.emit_list(node.span(), Some(&node.cases), ListFormat::CaseBlockClauses)?;
        punct!("}");
    }

    #[emitter]
    pub fn emit_catch_clause(&mut self, node: &CatchClause) -> Result {
        keyword!("catch");
        space!();

        punct!("(");
        emit!(node.param);
        punct!(")");

        space!();

        emit!(node.body);
    }

    #[emitter]
    pub fn emit_switch_case(&mut self, node: &SwitchCase) -> Result {
        if let Some(ref test) = node.test {
            keyword!("case");
            space!();
            emit!(test);
        } else {
            keyword!("default");
        }

        let emit_as_single_stmt = node.cons.len() == 1 && {
            // treat synthesized nodes as located on the same line for emit purposes
            node.is_synthesized()
                || node.cons[0].is_synthesized()
                || self
                    .cm
                    .is_on_same_line(node.span().lo(), node.cons[0].span().lo())
        };

        let mut format = ListFormat::CaseOrDefaultClauseStatements;
        if emit_as_single_stmt {
            punct!(":");
            space!();
            format &= !(ListFormat::MultiLine | ListFormat::Indented);
        } else {
            punct!(":");
        }
        self.emit_list(node.span(), Some(&node.cons), format)?;
    }

    #[emitter]
    pub fn emit_throw_stmt(&mut self, node: &ThrowStmt) -> Result {
        keyword!("throw");
        space!();
        emit!(node.arg);
        semi!();
    }

    #[emitter]
    pub fn emit_try_stmt(&mut self, node: &TryStmt) -> Result {
        keyword!("try");
        // space!();
        emit!(node.block);

        if let Some(ref catch) = node.handler {
            // self.write_line_or_space(node.span())?;
            emit!(catch);
        }

        if let Some(ref finally) = node.finalizer {
            // self.write_line_or_space(node.span())?;
            keyword!("finally");
            // space!();
            emit!(finally);
        }
    }

    #[emitter]
    pub fn emit_while_stmt(&mut self, node: &WhileStmt) -> Result {
        keyword!("while");

        punct!("(");
        emit!(node.test);
        punct!(")");

        emit!(node.body);
    }

    #[emitter]
    pub fn emit_do_while_stmt(&mut self, node: &DoWhileStmt) -> Result {
        keyword!("do");
        space!();
        emit!(node.body);

        keyword!("while");
        space!();
        emit!(node.test);
    }

    #[emitter]
    pub fn emit_for_stmt(&mut self, node: &ForStmt) -> Result {
        keyword!("for");
        punct!("(");
        opt!(node.init);
        semi!();
        opt_leading_space!(node.test);
        semi!();
        opt_leading_space!(node.update);
        punct!(")");

        emit!(node.body);
    }

    #[emitter]
    pub fn emit_for_in_stmt(&mut self, node: &ForInStmt) -> Result {
        keyword!("for");
        punct!("(");
        emit!(node.left);
        space!();
        keyword!("in");
        space!();
        emit!(node.right);
        punct!(")");

        emit!(node.body);
    }

    #[emitter]
    pub fn emit_for_of_stmt(&mut self, node: &ForOfStmt) -> Result {
        keyword!("for");
        punct!("(");
        emit!(node.left);
        space!();
        keyword!("of");
        space!();
        emit!(node.right);
        punct!(")");
        emit!(node.body);
    }
}

impl<'a> Emitter<'a> {
    fn write_delim(&mut self, f: ListFormat) -> Result {
        match f & ListFormat::DelimitersMask {
            ListFormat::None => {}
            ListFormat::CommaDelimited => self.wr.write_punct(",")?,
            ListFormat::BarDelimited => {
                self.wr.write_space()?;
                self.wr.write_punct("|")?;
            }
            ListFormat::AmpersandDelimited => {
                self.wr.write_space()?;
                self.wr.write_punct("&")?;
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    #[emitter]
    pub fn emit_var_decl_or_expr(&mut self, node: &VarDeclOrExpr) -> Result {
        match *node {
            VarDeclOrExpr::Expr(ref node) => emit!(node),
            VarDeclOrExpr::VarDecl(ref node) => emit!(node),
        }
    }
}

fn get_text_of_node<T: Spanned>(
    cm: &Lrc<SourceMap>,
    node: &T,
    _include_travia: bool,
) -> Option<String> {
    let span = node.span();
    if span.is_dummy() || span.ctxt() != SyntaxContext::empty() {
        // This node is transformed so we shoukld not use original source code.
        return None;
    }

    let s = cm.span_to_snippet(span).unwrap();
    if s == "" {
        return None;
    }
    Some(s)

    // let span = node.span();
    // let src = match file.src {
    //     Some(ref src) => src.clone(),
    //     None => return None,
    // };
    // assert!(file.contains(span.lo()));
    // assert!(file.contains(span.hi()));

    // let lo: u32 = span.lo().0 - file.start_pos.0;
    // let hi: u32 = span.hi().0 - file.start_pos.0;

    // let s: &str = &src;
    // let s = &s[lo as usize..hi as usize];
    // Some(s.to_string())
}

/// In some cases, we need to emit a space between the operator and the operand.
/// One obvious case is when the operator is an identifier, like delete or
/// typeof. We also need to do this for plus and minus expressions in certain
/// cases. Specifically, consider the following two cases (parens are just for
/// clarity of exposition, and not part of the source code):
///
///  (+(+1))
///  (+(++1))
///
/// We need to emit a space in both cases. In the first case, the absence of a
/// space will make the resulting expression a prefix increment operation. And
/// in the second, it will make the resulting expression a prefix increment
/// whose operand is a plus expression - (++(+x)) The same is true of minus of
/// course.
fn should_emit_whitespace_before_operand(node: &UnaryExpr) -> bool {
    match *node {
        UnaryExpr {
            op: op!("void"), ..
        }
        | UnaryExpr {
            op: op!("typeof"), ..
        }
        | UnaryExpr {
            op: op!("delete"), ..
        } => return node.arg.starts_with_alpha_num(),
        _ => {}
    }

    match *node.arg {
        Expr::Update(UpdateExpr {
            op: op!("++"),
            prefix: true,
            ..
        })
        | Expr::Unary(UnaryExpr {
            op: op!(unary, "+"),
            ..
        }) if node.op == op!(unary, "+") => true,
        Expr::Update(UpdateExpr {
            op: op!("--"),
            prefix: true,
            ..
        })
        | Expr::Unary(UnaryExpr {
            op: op!(unary, "-"),
            ..
        }) if node.op == op!(unary, "-") => true,
        _ => false,
    }
}

impl<N> Node for Option<N>
where
    N: Node,
{
    fn emit_with(&self, e: &mut Emitter) -> Result {
        match *self {
            Some(ref n) => n.emit_with(e),
            None => Ok(()),
        }
    }
}