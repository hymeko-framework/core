use crate::common::ids::SymId;
use crate::resolution::interner::Interner;
use crate::sym_ast::{AstSym, DeclarationFields};
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
    let ast = intern_ast_into(src, &mut it);
    Interned { ast, interner: it }
}

pub fn intern_ast_into<'a>(src: &AstStr<'a>, it: &mut Interner) -> AstSym<'static> {
    AstInterner { it }.desc(src)
}

/// Owns symbol allocation order across the complete AST traversal.
struct AstInterner<'i> {
    it: &'i mut Interner,
}

impl AstInterner<'_> {
    fn desc<'a>(&mut self, src: &Description<'a, &'a str>) -> Description<'static, SymId> {
        Description {
            name: self.it.intern(src.name),
            header: src.header.iter().map(|n| self.node(n)).collect(),
            imports: src.imports.iter().map(|imp| self.import(imp)).collect(),
            usings: src.usings.iter().map(|u| self.using(u)).collect(),
            consts: src.consts.iter().map(|c| self.const_decl(c)).collect(),
            items: src.items.iter().map(|x| self.item(x)).collect(),
        }
    }

    fn const_decl(&mut self, src: &ConstDecl<&str>) -> ConstDecl<SymId> {
        ConstDecl {
            name: self.it.intern(src.name),
            value: self.const_expr(&src.value),
        }
    }

    fn const_expr(&mut self, src: &ConstExpr<&str>) -> ConstExpr<SymId> {
        match src {
            ConstExpr::Lit(n) => ConstExpr::Lit(*n),
            ConstExpr::Ref(name) => ConstExpr::Ref(self.it.intern(name)),
            ConstExpr::Neg(e) => ConstExpr::Neg(Box::new(self.const_expr(e))),
            ConstExpr::Bin(op, l, r) => ConstExpr::Bin(
                *op,
                Box::new(self.const_expr(l)),
                Box::new(self.const_expr(r)),
            ),
            ConstExpr::Pi => ConstExpr::Pi,
            ConstExpr::Exp(e) => ConstExpr::Exp(Box::new(self.const_expr(e))),
        }
    }

    fn using(&mut self, src: &UsingStmt<&str>) -> UsingStmt<SymId> {
        UsingStmt {
            path: Ref {
                path: src.path.path.iter().map(|&p| self.it.intern(p)).collect(),
            },
            alias: self.it.intern(src.alias),
        }
    }

    fn item<'a>(&mut self, src: &HyperItem<'a, &'a str>) -> HyperItem<'static, SymId> {
        match src {
            HyperItem::Node(n) => HyperItem::Node(self.node(n)),
            HyperItem::Edge(e) => HyperItem::Edge(self.edge(e)),
            HyperItem::Arc(a) => HyperItem::Arc(self.arc(a)),
        }
    }

    fn anno<'a>(&mut self, src: &Anno<'a, &'a str>) -> Anno<'static, SymId> {
        Anno {
            tags: src.tags.iter().map(|&t| self.it.intern(t)).collect(),
            value: src.value.as_ref().map(|v| self.value(v)),
        }
    }

    fn arc<'a>(&mut self, src: &HyperArc<'a, &'a str>) -> HyperArc<'static, SymId> {
        HyperAnnotatedElement {
            anno: self.anno(&src.anno),
            inner: ArcInner {
                refs: src.inner.refs.iter().map(|r| self.signed_ref(r)).collect(),
            },
        }
    }

    fn signed_ref<'a>(&mut self, src: &SignedRef<'a, &'a str>) -> SignedRef<'static, SymId> {
        match src {
            SignedRef::Plus(a) => SignedRef::Plus(self.ref_atom(a)),
            SignedRef::Minus(a) => SignedRef::Minus(self.ref_atom(a)),
            SignedRef::Neutral(a) => SignedRef::Neutral(self.ref_atom(a)),
        }
    }

    fn ref_atom<'a>(&mut self, src: &RefAtom<'a, &'a str>) -> RefAtom<'static, SymId> {
        RefAtom {
            target: Ref {
                path: src.target.path.iter().map(|&p| self.it.intern(p)).collect(),
            },
            anno: self.anno(&src.anno),
        }
    }

    fn import<'a>(&mut self, src: &ImportStmt<'a, &'a str>) -> ImportStmt<'static, SymId> {
        ImportStmt {
            path: Cow::Owned(src.path.as_ref().to_string()),
            alias: src.alias.map(|n| self.it.intern(n)),
        }
    }

    fn value<'a>(&mut self, src: &Value<'a, &'a str>) -> Value<'static, SymId> {
        match src {
            Value::Str(s) => Value::Str(Cow::Owned(s.as_ref().to_string())),
            Value::Num(n) => Value::Num(*n),
            Value::List(xs) => Value::List(xs.iter().map(|v| self.value(v)).collect()),
            Value::Ref(r) => Value::Ref(Ref {
                path: r.path.iter().map(|&seg| self.it.intern(seg)).collect(),
            }),
            Value::Expr(e) => Value::Expr(self.const_expr(e)),
        }
    }
    fn declaration<'a, T: DeclarationFields<'a, &'a str>, U>(
        &mut self,
        src: &HyperAnnotatedElement<'a, T, &'a str>,
        build: impl FnOnce(
            SymId,
            Vec<SignedRef<'static, SymId>>,
            Option<Vec<HyperItem<'static, SymId>>>,
        ) -> U,
    ) -> HyperAnnotatedElement<'static, U, SymId> {
        let anno = self.anno(&src.anno);
        let (name, bases, body) = src.inner.fields();
        let name = self.it.intern(name);
        let bases = bases.iter().map(|r| self.signed_ref(r)).collect();
        let body = body.map(|xs| xs.iter().map(|x| self.item(x)).collect());
        HyperAnnotatedElement {
            anno,
            inner: build(name, bases, body),
        }
    }

    fn node<'a>(&mut self, src: &NodeDecl<'a, &'a str>) -> NodeDecl<'static, SymId> {
        self.declaration(src, |name, bases, body| NodeInner { name, bases, body })
    }

    fn edge<'a>(&mut self, src: &EdgeDecl<'a, &'a str>) -> EdgeDecl<'static, SymId> {
        self.declaration(src, |name, bases, body| EdgeInner {
            name,
            bases,
            body: body.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_points_share_symbol_order_and_own_text_after_source_is_dropped() {
        let source = String::from(
            r#"description {
            @"vocabulary.hymeko" -> vocabulary;
            using vocabulary.base as kind;
            const B = 2.0;
            const A = -B + exp(0.0) * pi;
            header <tag> "header text";
        }
        root: kind { value ["text", (A / B), root]; @edge: kind { (+root [1.0], -value, ~header); } }
        "#,
        );
        let ast = parser::parse_description(&source).unwrap();
        let first = intern_ast(&ast);
        let mut owned_it = Interner::new();
        let owned = intern_ast_into(&ast, &mut owned_it);
        let expected = format!("{:?}", first.ast);
        let symbols: Vec<_> = first.interner.iter().collect();
        assert_eq!(symbols, owned_it.iter().collect::<Vec<_>>());
        assert_eq!(
            symbols.iter().map(|(_, text)| *text).collect::<Vec<_>>(),
            [
                "description",
                "tag",
                "header",
                "vocabulary",
                "base",
                "kind",
                "B",
                "A",
                "root",
                "value",
                "edge"
            ]
        );
        assert!(owned.header[0].inner.body.is_none());
        let HyperItem::Node(root) = &owned.items[0] else {
            panic!("root node required")
        };
        assert!(root.inner.body.is_some());

        drop(ast);
        drop(source);
        assert_eq!(format!("{owned:?}"), expected);
        assert!(matches!(owned.imports[0].path, Cow::Owned(_)));
        assert!(matches!(
            owned.header[0].anno.value,
            Some(Value::Str(Cow::Owned(_)))
        ));
    }
}
