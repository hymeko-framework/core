use crate::common::ids::SymId;
use crate::resolution::interner::Interner;
use crate::sym_ast::AstSym;
use parser::ast::{
    Anno, ArcInner, AstStr, ConstDecl, ConstExpr, Description, EdgeDecl, EdgeInner,
    HyperAnnotatedElement, HyperArc, HyperItem, ImportStmt, NodeDecl, NodeInner, Ref, RefAtom,
    SignedRef, UsingStmt, Value,
};
use std::borrow::Cow;

pub struct Interned<'a> {
    pub ast: AstSym<'a>,
    pub interner: Interner,
}

pub fn intern_ast<'a>(src: &AstStr<'a>) -> Interned<'a> {
    let mut it = Interner::new();
    let ast = lower_desc(src, &mut it);
    Interned { ast, interner: it }
}

fn sid(it: &mut Interner, s: &str) -> SymId {
    it.intern(s)
}

fn lower_desc<'a>(src: &Description<'a, &'a str>, it: &mut Interner) -> Description<'a, SymId> {
    Description {
        name: sid(it, src.name),
        header: src.header.iter().map(|n| lower_node(n, it)).collect(),
        imports: src
            .imports
            .iter()
            .map(|imp| lower_import(imp, it))
            .collect(),
        usings: src.usings.iter().map(|u| lower_using(u, it)).collect(),
        consts: src.consts.iter().map(|c| lower_const_decl(c, it)).collect(),
        items: src.items.iter().map(|x| lower_item(x, it)).collect(),
    }
}

fn lower_const_decl(src: &ConstDecl<&str>, it: &mut Interner) -> ConstDecl<SymId> {
    ConstDecl {
        name: sid(it, src.name),
        value: lower_const_expr(&src.value, it),
    }
}

fn lower_const_expr(src: &ConstExpr<&str>, it: &mut Interner) -> ConstExpr<SymId> {
    match src {
        ConstExpr::Lit(n) => ConstExpr::Lit(*n),
        ConstExpr::Ref(name) => ConstExpr::Ref(sid(it, name)),
        ConstExpr::Neg(e) => ConstExpr::Neg(Box::new(lower_const_expr(e, it))),
        ConstExpr::Bin(op, l, r) => ConstExpr::Bin(
            *op,
            Box::new(lower_const_expr(l, it)),
            Box::new(lower_const_expr(r, it)),
        ),
        ConstExpr::Pi => ConstExpr::Pi,
        ConstExpr::Exp(e) => ConstExpr::Exp(Box::new(lower_const_expr(e, it))),
    }
}

fn lower_using(src: &UsingStmt<&str>, it: &mut Interner) -> UsingStmt<SymId> {
    UsingStmt {
        path: Ref {
            path: src.path.path.iter().map(|&p| sid(it, p)).collect(),
        },
        alias: sid(it, src.alias),
    }
}

fn lower_item<'a>(src: &HyperItem<'a, &'a str>, it: &mut Interner) -> HyperItem<'a, SymId> {
    match src {
        HyperItem::Node(n) => HyperItem::Node(lower_node(n, it)),
        HyperItem::Edge(e) => HyperItem::Edge(lower_edge(e, it)),
        HyperItem::Arc(a) => HyperItem::Arc(lower_arc(a, it)),
    }
}

fn lower_anno<'a>(src: &Anno<'a, &'a str>, it: &mut Interner) -> Anno<'a, SymId> {
    Anno {
        tags: src.tags.iter().map(|&t| sid(it, t)).collect(),
        value: src.value.as_ref().map(|v| lower_value(v, it)),
    }
}

fn lower_node<'a>(src: &NodeDecl<'a, &'a str>, it: &mut Interner) -> NodeDecl<'a, SymId> {
    HyperAnnotatedElement {
        anno: lower_anno(&src.anno, it),
        inner: NodeInner {
            name: sid(it, src.inner.name),
            bases: src
                .inner
                .bases
                .iter()
                .map(|r| lower_signed_ref(r, it))
                .collect(),
            body: src
                .inner
                .body
                .as_ref()
                .map(|xs| xs.iter().map(|x| lower_item(x, it)).collect()),
        },
    }
}

fn lower_edge<'a>(src: &EdgeDecl<'a, &'a str>, it: &mut Interner) -> EdgeDecl<'a, SymId> {
    HyperAnnotatedElement {
        anno: lower_anno(&src.anno, it),
        inner: EdgeInner {
            name: sid(it, src.inner.name),
            bases: src
                .inner
                .bases
                .iter()
                .map(|r| lower_signed_ref(r, it))
                .collect(),
            body: src.inner.body.iter().map(|x| lower_item(x, it)).collect(),
        },
    }
}

fn lower_arc<'a>(src: &HyperArc<'a, &'a str>, it: &mut Interner) -> HyperArc<'a, SymId> {
    HyperAnnotatedElement {
        anno: lower_anno(&src.anno, it),
        inner: ArcInner {
            refs: src
                .inner
                .refs
                .iter()
                .map(|r| lower_signed_ref(r, it))
                .collect(),
        },
    }
}

fn lower_signed_ref<'a>(src: &SignedRef<'a, &'a str>, it: &mut Interner) -> SignedRef<'a, SymId> {
    match src {
        SignedRef::Plus(a) => SignedRef::Plus(lower_ref_atom(a, it)),
        SignedRef::Minus(a) => SignedRef::Minus(lower_ref_atom(a, it)),
        SignedRef::Neutral(a) => SignedRef::Neutral(lower_ref_atom(a, it)),
    }
}

fn lower_ref_atom<'a>(src: &RefAtom<'a, &'a str>, it: &mut Interner) -> RefAtom<'a, SymId> {
    RefAtom {
        target: Ref {
            path: src.target.path.iter().map(|&p| sid(it, p)).collect(),
        },
        anno: lower_anno(&src.anno, it),
    }
}

fn lower_import<'a>(src: &ImportStmt<'a, &'a str>, it: &mut Interner) -> ImportStmt<'a, SymId> {
    ImportStmt {
        path: Cow::Owned(src.path.as_ref().to_string()),
        alias: src.alias.map(|n| sid(it, n)),
    }
}

fn lower_value<'a>(src: &Value<'a, &'a str>, it: &mut Interner) -> Value<'a, SymId> {
    match src {
        Value::Str(s) => Value::Str(Cow::Owned(s.as_ref().to_string())),
        Value::Num(n) => Value::Num(*n),
        Value::List(xs) => Value::List(xs.iter().map(|v| lower_value(v, it)).collect()),
        Value::Ref(r) => Value::Ref(Ref {
            path: r.path.iter().map(|&seg| sid(it, seg)).collect(),
        }),
        Value::Expr(e) => Value::Expr(lower_const_expr(e, it)),
    }
}

pub fn intern_ast_into<'a>(src: &AstStr<'a>, it: &mut Interner) -> AstSym<'a> {
    lower_desc(src, it)
}

pub fn intern_ast_into_owned<'a>(src: &AstStr<'a>, it: &mut Interner) -> AstSym<'static> {
    lower_desc_owned(src, it)
}

fn lower_desc_owned<'a>(
    src: &Description<'a, &'a str>,
    it: &mut Interner,
) -> Description<'static, SymId> {
    Description {
        name: sid(it, src.name),
        header: src.header.iter().map(|n| lower_node_owned(n, it)).collect(),
        imports: src
            .imports
            .iter()
            .map(|imp| lower_import_owned(imp, it))
            .collect(),
        usings: src
            .usings
            .iter()
            .map(|u| lower_using_owned(u, it))
            .collect(),
        consts: src.consts.iter().map(|c| lower_const_decl(c, it)).collect(),
        items: src.items.iter().map(|x| lower_item_owned(x, it)).collect(),
    }
}

fn lower_using_owned(src: &UsingStmt<&str>, it: &mut Interner) -> UsingStmt<SymId> {
    UsingStmt {
        path: Ref {
            path: src.path.path.iter().map(|&p| sid(it, p)).collect(),
        },
        alias: sid(it, src.alias),
    }
}

fn lower_item_owned<'a>(
    src: &HyperItem<'a, &'a str>,
    it: &mut Interner,
) -> HyperItem<'static, SymId> {
    match src {
        HyperItem::Node(n) => HyperItem::Node(lower_node_owned(n, it)),
        HyperItem::Edge(e) => HyperItem::Edge(lower_edge_owned(e, it)),
        HyperItem::Arc(a) => HyperItem::Arc(lower_arc_owned(a, it)),
    }
}

fn lower_anno_owned<'a>(src: &Anno<'a, &'a str>, it: &mut Interner) -> Anno<'static, SymId> {
    Anno {
        tags: src.tags.iter().map(|&t| sid(it, t)).collect(),
        value: src.value.as_ref().map(|v| lower_value_owned(v, it)),
    }
}

fn lower_node_owned<'a>(
    src: &NodeDecl<'a, &'a str>,
    it: &mut Interner,
) -> NodeDecl<'static, SymId> {
    HyperAnnotatedElement {
        anno: lower_anno_owned(&src.anno, it),
        inner: NodeInner {
            name: sid(it, src.inner.name),
            bases: src
                .inner
                .bases
                .iter()
                .map(|r| lower_signed_ref_owned(r, it))
                .collect(),
            body: src
                .inner
                .body
                .as_ref()
                .map(|xs| xs.iter().map(|x| lower_item_owned(x, it)).collect()),
        },
    }
}

fn lower_edge_owned<'a>(
    src: &EdgeDecl<'a, &'a str>,
    it: &mut Interner,
) -> EdgeDecl<'static, SymId> {
    HyperAnnotatedElement {
        anno: lower_anno_owned(&src.anno, it),
        inner: EdgeInner {
            name: sid(it, src.inner.name),
            bases: src
                .inner
                .bases
                .iter()
                .map(|r| lower_signed_ref_owned(r, it))
                .collect(),
            body: src
                .inner
                .body
                .iter()
                .map(|x| lower_item_owned(x, it))
                .collect(),
        },
    }
}

fn lower_arc_owned<'a>(src: &HyperArc<'a, &'a str>, it: &mut Interner) -> HyperArc<'static, SymId> {
    HyperAnnotatedElement {
        anno: lower_anno_owned(&src.anno, it),
        inner: ArcInner {
            refs: src
                .inner
                .refs
                .iter()
                .map(|r| lower_signed_ref_owned(r, it))
                .collect(),
        },
    }
}

fn lower_signed_ref_owned<'a>(
    src: &SignedRef<'a, &'a str>,
    it: &mut Interner,
) -> SignedRef<'static, SymId> {
    match src {
        SignedRef::Plus(a) => SignedRef::Plus(lower_ref_atom_owned(a, it)),
        SignedRef::Minus(a) => SignedRef::Minus(lower_ref_atom_owned(a, it)),
        SignedRef::Neutral(a) => SignedRef::Neutral(lower_ref_atom_owned(a, it)),
    }
}

fn lower_ref_atom_owned<'a>(
    src: &RefAtom<'a, &'a str>,
    it: &mut Interner,
) -> RefAtom<'static, SymId> {
    RefAtom {
        target: Ref {
            path: src.target.path.iter().map(|&p| sid(it, p)).collect(),
        },
        anno: lower_anno_owned(&src.anno, it),
    }
}

fn lower_import_owned<'a>(
    src: &ImportStmt<'a, &'a str>,
    it: &mut Interner,
) -> ImportStmt<'static, SymId> {
    ImportStmt {
        path: Cow::Owned(src.path.as_ref().to_string()),
        alias: src.alias.map(|n| sid(it, n)),
    }
}

fn lower_value_owned<'a>(src: &Value<'a, &'a str>, it: &mut Interner) -> Value<'static, SymId> {
    match src {
        Value::Str(s) => Value::Str(Cow::Owned(s.as_ref().to_string())),
        Value::Num(n) => Value::Num(*n),
        Value::List(xs) => Value::List(xs.iter().map(|v| lower_value_owned(v, it)).collect()),
        Value::Ref(r) => Value::Ref(Ref {
            path: r.path.iter().map(|&seg| sid(it, seg)).collect(),
        }),
        Value::Expr(e) => Value::Expr(lower_const_expr(e, it)),
    }
}
