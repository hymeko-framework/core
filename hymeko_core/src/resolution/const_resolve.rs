//! Tier B: compile-time numeric-expression resolver.
//!
//! After interning, the AST may contain `Value::Expr(ConstExpr)` nodes
//! (parenthesised numeric expressions in value positions) and
//! `Value::Ref(<single-segment>)` paths whose single segment matches a
//! `const` decl in the description header. Both forms must be reduced to
//! `Value::Num(f64)` before the resolve / lower pipeline runs --- the
//! IR is purely numeric in those positions.
//!
//! The resolver is a two-pass design with a topological evaluation
//! order, so forward references between consts are allowed:
//!
//! ```text
//! const RADIUS = LEN / 2.0;
//! const LEN    = 0.1;
//! ```
//!
//! evaluates fine. Cycles are detected and reported.

use crate::sym_ast::DeclarationFields;
use std::collections::HashMap;

use parser::ast::{
    Anno, BinOp, ConstDecl, ConstExpr, Description, HyperAnnotatedElement, HyperItem, SignedRef,
    Value,
};

use crate::common::ids::SymId;
use crate::resolution::interner::Interner;

#[derive(Debug, Clone, PartialEq)]
pub enum ConstResolveError {
    /// A `const` reference inside an expression (or a value-position
    /// single-segment ref) names an identifier that is not in scope as
    /// a const decl.
    UndefinedRef { name: String },
    /// A cycle between two or more const decls.
    Cycle { names: Vec<String> },
    /// Division by zero in a const expression.
    DivisionByZero,
    /// `exp` (or future builtins) called with the wrong arity. With
    /// the current grammar this is structurally impossible — the
    /// parser only produces `Exp(Box<...>)` from `exp(<expr>)` — but
    /// the variant is present for forward-compatibility.
    BadCall { what: String },
}

impl std::fmt::Display for ConstResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UndefinedRef { name } => write!(f, "undefined const reference `{name}`"),
            Self::Cycle { names } => write!(f, "cyclic const definitions: {}", names.join(" → ")),
            Self::DivisionByZero => write!(f, "division by zero in const expression"),
            Self::BadCall { what } => write!(f, "bad const call: {what}"),
        }
    }
}

impl std::error::Error for ConstResolveError {}

/// Build a `name → f64` table by topologically evaluating the const
/// decls. Forward references are supported via memoized DFS.
pub fn evaluate_consts(
    decls: &[ConstDecl<SymId>],
    it: &Interner,
) -> Result<HashMap<SymId, f64>, ConstResolveError> {
    let mut definitions = ConstDefinitions {
        by_name: decls.iter().map(|d| (d.name, &d.value)).collect(),
        resolved: HashMap::with_capacity(decls.len()),
        in_progress: Vec::new(),
        it,
    };
    // Source order determines the first diagnostic, even for unused constants.
    for decl in decls {
        definitions.resolve(decl.name)?;
    }
    Ok(definitions.resolved)
}

/// One arithmetic traversal; strategies own dependency evaluation or table lookup.
trait ConstReferences {
    fn resolve(&mut self, name: SymId) -> Result<f64, ConstResolveError>;

    fn evaluate(&mut self, expr: &ConstExpr<SymId>) -> Result<f64, ConstResolveError> {
        Ok(match expr {
            ConstExpr::Lit(n) => *n,
            ConstExpr::Pi => std::f64::consts::PI,
            ConstExpr::Exp(arg) => self.evaluate(arg)?.exp(),
            ConstExpr::Neg(arg) => -self.evaluate(arg)?,
            ConstExpr::Ref(name) => self.resolve(*name)?,
            ConstExpr::Bin(op, left, right) => {
                let left = self.evaluate(left)?;
                let right = self.evaluate(right)?;
                match op {
                    BinOp::Add => left + right,
                    BinOp::Sub => left - right,
                    BinOp::Mul => left * right,
                    BinOp::Div if right == 0.0 => return Err(ConstResolveError::DivisionByZero),
                    BinOp::Div => left / right,
                }
            }
        })
    }
}

struct ConstDefinitions<'a> {
    by_name: HashMap<SymId, &'a ConstExpr<SymId>>,
    resolved: HashMap<SymId, f64>,
    in_progress: Vec<SymId>,
    it: &'a Interner,
}

impl ConstReferences for ConstDefinitions<'_> {
    fn resolve(&mut self, sym: SymId) -> Result<f64, ConstResolveError> {
        if let Some(&value) = self.resolved.get(&sym) {
            return Ok(value);
        }
        if let Some(start) = self.in_progress.iter().position(|&s| s == sym) {
            let names = self.in_progress[start..]
                .iter()
                .copied()
                .chain([sym])
                .map(|s| self.it.resolve(s).to_owned())
                .collect();
            return Err(ConstResolveError::Cycle { names });
        }
        let expr = *self
            .by_name
            .get(&sym)
            .ok_or_else(|| ConstResolveError::UndefinedRef {
                name: self.it.resolve(sym).to_owned(),
            })?;
        self.in_progress.push(sym);
        let result = self.evaluate(expr);
        self.in_progress.pop();
        let value = result?;
        self.resolved.insert(sym, value);
        Ok(value)
    }
}

struct ConstValues<'a> {
    table: &'a HashMap<SymId, f64>,
    it: &'a Interner,
}

impl ConstReferences for ConstValues<'_> {
    fn resolve(&mut self, sym: SymId) -> Result<f64, ConstResolveError> {
        self.table
            .get(&sym)
            .copied()
            .ok_or_else(|| ConstResolveError::UndefinedRef {
                name: self.it.resolve(sym).to_owned(),
            })
    }
}

/// Walk the description tree, substituting every `Value::Expr` with its
/// evaluated `Value::Num`, and every `Value::Ref` whose path is a
/// single segment matching a const decl with the const's value.
/// Leaves multi-segment refs alone (they are real cross-decl
/// references and resolved by `resolve.rs`).
pub fn resolve_consts<'a>(
    desc: &mut Description<'a, SymId>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    let table = evaluate_consts(&desc.consts, it)?;
    let mut values = ConstValues { table: &table, it };
    for node in &mut desc.header {
        values.declaration(node)?;
    }
    for item in &mut desc.items {
        values.item(item)?;
    }
    Ok(())
}

impl ConstValues<'_> {
    fn item(&mut self, item: &mut HyperItem<'_, SymId>) -> Result<(), ConstResolveError> {
        match item {
            HyperItem::Node(n) => self.declaration(n),
            HyperItem::Edge(e) => self.declaration(e),
            HyperItem::Arc(a) => {
                // Arc references precede the arc annotation in diagnostic order.
                for reference in &mut a.inner.refs {
                    self.signed_ref(reference)?;
                }
                self.anno(&mut a.anno)
            }
        }
    }

    fn declaration<'a, T: DeclarationFields<'a, SymId>>(
        &mut self,
        declaration: &mut HyperAnnotatedElement<'a, T, SymId>,
    ) -> Result<(), ConstResolveError> {
        self.anno(&mut declaration.anno)?;
        let (bases, body) = declaration.inner.fields_mut();
        for reference in bases {
            self.signed_ref(reference)?;
        }
        for item in body {
            self.item(item)?;
        }
        Ok(())
    }

    fn signed_ref(
        &mut self,
        reference: &mut SignedRef<'_, SymId>,
    ) -> Result<(), ConstResolveError> {
        let atom = match reference {
            SignedRef::Plus(a) | SignedRef::Minus(a) | SignedRef::Neutral(a) => a,
        };
        self.anno(&mut atom.anno)
    }

    fn anno(&mut self, anno: &mut Anno<'_, SymId>) -> Result<(), ConstResolveError> {
        if let Some(value) = &mut anno.value {
            self.value(value)?;
        }
        Ok(())
    }

    fn value(&mut self, value: &mut Value<'_, SymId>) -> Result<(), ConstResolveError> {
        match value {
            Value::Str(_) | Value::Num(_) => {}
            Value::List(items) => {
                for item in items {
                    self.value(item)?;
                }
            }
            Value::Ref(reference) => {
                // Multi-segment paths remain references for the name resolver.
                if reference.path.len() == 1 {
                    if let Some(&number) = self.table.get(&reference.path[0]) {
                        *value = Value::Num(number);
                    }
                }
            }
            Value::Expr(expr) => {
                *value = Value::Num(self.evaluate(expr)?);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolution::interner::Interner;

    fn mk_lit(n: f64) -> ConstExpr<SymId> {
        ConstExpr::Lit(n)
    }
    fn mk_bin(op: BinOp, l: ConstExpr<SymId>, r: ConstExpr<SymId>) -> ConstExpr<SymId> {
        ConstExpr::Bin(op, Box::new(l), Box::new(r))
    }

    struct SubstitutionCase {
        item: HyperItem<'static, SymId>,
        expected: &'static str,
    }

    impl SubstitutionCase {
        fn check(mut self, values: &mut ConstValues<'_>) {
            assert_eq!(
                values.item(&mut self.item),
                Err(ConstResolveError::UndefinedRef {
                    name: self.expected.into(),
                })
            );
        }
    }

    #[test]
    fn declaration_and_arc_diagnostics_keep_traversal_order() {
        use parser::ast::{ArcInner, EdgeInner, NodeInner, Ref, RefAtom};
        let mut it = Interner::new();
        let name = it.intern("name");
        let annotation = it.intern("annotation");
        let base = it.intern("base");
        let child = it.intern("child");
        let anno = |sym| Anno {
            tags: vec![],
            value: Some(Value::Expr(ConstExpr::Ref(sym))),
        };
        let reference = SignedRef::Minus(RefAtom {
            target: Ref { path: vec![name] },
            anno: anno(base),
        });
        let body = vec![HyperItem::Node(HyperAnnotatedElement {
            anno: anno(child),
            inner: NodeInner {
                name,
                bases: vec![],
                body: None,
            },
        })];
        let table = HashMap::new();
        let mut values = ConstValues {
            table: &table,
            it: &it,
        };
        for (annotation_value, bases, expected) in [
            (
                Some(Value::Expr(ConstExpr::Ref(annotation))),
                vec![reference.clone()],
                "annotation",
            ),
            (None, vec![reference.clone()], "base"),
            (None, vec![], "child"),
        ] {
            let annotation = Anno {
                tags: vec![],
                value: annotation_value,
            };
            for item in [
                HyperItem::Node(HyperAnnotatedElement {
                    anno: annotation.clone(),
                    inner: NodeInner {
                        name,
                        bases: bases.clone(),
                        body: Some(body.clone()),
                    },
                }),
                HyperItem::Edge(HyperAnnotatedElement {
                    anno: annotation,
                    inner: EdgeInner {
                        name,
                        bases: bases.clone(),
                        body: body.clone(),
                    },
                }),
            ] {
                SubstitutionCase { item, expected }.check(&mut values);
            }
        }
        SubstitutionCase {
            item: HyperItem::Arc(HyperAnnotatedElement {
                anno: anno(annotation),
                inner: ArcInner {
                    refs: vec![reference],
                },
            }),
            expected: "base",
        }
        .check(&mut values);
    }

    #[test]
    fn evaluates_simple_literals() {
        let mut it = Interner::default();
        let a = it.intern("A");
        let decls = vec![ConstDecl {
            name: a,
            value: mk_lit(0.5),
        }];
        let table = evaluate_consts(&decls, &it).unwrap();
        assert_eq!(table.get(&a), Some(&0.5));
    }

    #[test]
    fn evaluates_arithmetic() {
        let mut it = Interner::default();
        let a = it.intern("A");
        // A = 0.1 * 2.0 + 0.05
        let expr = mk_bin(
            BinOp::Add,
            mk_bin(BinOp::Mul, mk_lit(0.1), mk_lit(2.0)),
            mk_lit(0.05),
        );
        let decls = vec![ConstDecl {
            name: a,
            value: expr,
        }];
        let table = evaluate_consts(&decls, &it).unwrap();
        assert!((table[&a] - 0.25).abs() < 1e-12);
    }

    #[test]
    fn forward_references_resolve() {
        let mut it = Interner::default();
        let a = it.intern("A");
        let b = it.intern("B");
        // A = B / 2;  B = 1.0;
        let decls = vec![
            ConstDecl {
                name: a,
                value: mk_bin(BinOp::Div, ConstExpr::Ref(b), mk_lit(2.0)),
            },
            ConstDecl {
                name: b,
                value: mk_lit(1.0),
            },
        ];
        let table = evaluate_consts(&decls, &it).unwrap();
        assert_eq!(table.get(&a), Some(&0.5));
        assert_eq!(table.get(&b), Some(&1.0));
    }

    #[test]
    fn cycle_detected() {
        let mut it = Interner::default();
        let a = it.intern("A");
        let b = it.intern("B");
        let decls = vec![
            ConstDecl {
                name: a,
                value: ConstExpr::Ref(b),
            },
            ConstDecl {
                name: b,
                value: ConstExpr::Ref(a),
            },
        ];
        let err = evaluate_consts(&decls, &it).unwrap_err();
        match err {
            ConstResolveError::Cycle { names } => {
                assert!(names.contains(&"A".to_string()));
                assert!(names.contains(&"B".to_string()));
            }
            _ => panic!("expected Cycle, got {err:?}"),
        }
    }

    #[test]
    fn undefined_ref_detected() {
        let mut it = Interner::default();
        let a = it.intern("A");
        let undefined = it.intern("UNDEFINED");
        let decls = vec![ConstDecl {
            name: a,
            value: ConstExpr::Ref(undefined),
        }];
        let err = evaluate_consts(&decls, &it).unwrap_err();
        match err {
            ConstResolveError::UndefinedRef { name } => assert_eq!(name, "UNDEFINED"),
            _ => panic!("expected UndefinedRef, got {err:?}"),
        }
    }

    #[test]
    fn division_by_zero_detected() {
        let mut it = Interner::default();
        let a = it.intern("A");
        let decls = vec![ConstDecl {
            name: a,
            value: mk_bin(BinOp::Div, mk_lit(1.0), mk_lit(0.0)),
        }];
        assert!(matches!(
            evaluate_consts(&decls, &it),
            Err(ConstResolveError::DivisionByZero)
        ));
    }

    #[test]
    fn pi_and_exp_evaluate() {
        let mut it = Interner::default();
        let a = it.intern("A");
        // A = exp(0) — should be 1.0
        let decls = vec![ConstDecl {
            name: a,
            value: ConstExpr::Exp(Box::new(mk_lit(0.0))),
        }];
        let table = evaluate_consts(&decls, &it).unwrap();
        assert!((table[&a] - 1.0).abs() < 1e-12);

        let b = it.intern("B");
        let decls = vec![ConstDecl {
            name: b,
            value: ConstExpr::Pi,
        }];
        let table = evaluate_consts(&decls, &it).unwrap();
        assert!((table[&b] - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn table_and_dependency_strategies_preserve_bits_and_first_error() {
        let mut it = Interner::new();
        let input = it.intern("input");
        let result = it.intern("result");
        let missing = it.intern("missing");
        let table = HashMap::from([(input, -0.0)]);
        let expressions = [
            ConstExpr::Ref(input),
            ConstExpr::Neg(Box::new(ConstExpr::Ref(input))),
            mk_bin(
                BinOp::Sub,
                ConstExpr::Pi,
                ConstExpr::Exp(Box::new(mk_lit(1.0))),
            ),
            mk_bin(BinOp::Div, mk_lit(-5.0), mk_lit(2.0)),
            mk_bin(
                BinOp::Add,
                mk_bin(BinOp::Add, mk_lit(1e16), mk_lit(-1e16)),
                mk_lit(1.0),
            ),
        ];
        for expr in expressions {
            let declarations = [
                ConstDecl {
                    name: result,
                    value: expr.clone(),
                },
                ConstDecl {
                    name: input,
                    value: mk_lit(-0.0),
                },
            ];
            let expected = evaluate_consts(&declarations, &it).unwrap()[&result];
            let mut value = Value::Expr(expr);
            ConstValues {
                table: &table,
                it: &it,
            }
            .value(&mut value)
            .unwrap();
            let Value::Num(actual) = value else {
                panic!("numeric substitution required")
            };
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        for (expr, expected) in [
            (
                mk_bin(BinOp::Div, ConstExpr::Ref(missing), mk_lit(0.0)),
                ConstResolveError::UndefinedRef {
                    name: "missing".into(),
                },
            ),
            (
                mk_bin(
                    BinOp::Add,
                    mk_bin(BinOp::Div, mk_lit(1.0), mk_lit(-0.0)),
                    ConstExpr::Ref(missing),
                ),
                ConstResolveError::DivisionByZero,
            ),
        ] {
            let declarations = [ConstDecl {
                name: result,
                value: expr.clone(),
            }];
            assert_eq!(evaluate_consts(&declarations, &it).unwrap_err(), expected);
            let mut value = Value::Expr(expr);
            let before = format!("{value:?}");
            assert_eq!(
                ConstValues {
                    table: &table,
                    it: &it
                }
                .value(&mut value),
                Err(expected)
            );
            assert_eq!(
                format!("{value:?}"),
                before,
                "failure must not replace the expression"
            );
        }
        assert_eq!(table.len(), 1);
        assert_eq!(table[&input].to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn shadowing_and_cycle_diagnostics_keep_source_order() {
        let mut it = Interner::new();
        let a = it.intern("a");
        let b = it.intern("b");
        let declarations = [
            ConstDecl {
                name: a,
                value: ConstExpr::Ref(b),
            },
            ConstDecl {
                name: a,
                value: mk_lit(7.0),
            },
        ];
        assert_eq!(evaluate_consts(&declarations, &it).unwrap()[&a], 7.0);
        let declarations = [
            ConstDecl {
                name: b,
                value: ConstExpr::Ref(a),
            },
            ConstDecl {
                name: a,
                value: ConstExpr::Ref(b),
            },
        ];
        assert_eq!(
            evaluate_consts(&declarations, &it).unwrap_err(),
            ConstResolveError::Cycle {
                names: vec!["b".into(), "a".into(), "b".into()]
            }
        );
    }
}
