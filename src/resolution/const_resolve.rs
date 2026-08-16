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

use std::collections::HashMap;

use parser::ast::{
    Anno, ArcInner, BinOp, ConstDecl, ConstExpr, Description, EdgeDecl, EdgeInner, HyperItem,
    NodeDecl, NodeInner, Ref, RefAtom, SignedRef, Value,
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
    // Build name → expr lookup. Last decl wins on shadowing (matches
    // the surface ordering — declared later overrides declared earlier).
    let mut by_name: HashMap<SymId, &ConstExpr<SymId>> = HashMap::with_capacity(decls.len());
    for d in decls {
        by_name.insert(d.name, &d.value);
    }

    let mut resolved: HashMap<SymId, f64> = HashMap::with_capacity(decls.len());
    let mut in_progress: Vec<SymId> = Vec::new(); // for cycle detection

    fn eval(
        sym: SymId,
        by_name: &HashMap<SymId, &ConstExpr<SymId>>,
        resolved: &mut HashMap<SymId, f64>,
        in_progress: &mut Vec<SymId>,
        it: &Interner,
    ) -> Result<f64, ConstResolveError> {
        if let Some(&v) = resolved.get(&sym) {
            return Ok(v);
        }
        if in_progress.contains(&sym) {
            // Build cycle path starting from `sym`'s first appearance.
            let start = in_progress.iter().position(|&s| s == sym).unwrap();
            let mut cycle: Vec<String> = in_progress[start..]
                .iter()
                .map(|s| it.resolve(*s).to_string())
                .collect();
            cycle.push(it.resolve(sym).to_string());
            return Err(ConstResolveError::Cycle { names: cycle });
        }
        let expr = by_name
            .get(&sym)
            .ok_or_else(|| ConstResolveError::UndefinedRef {
                name: it.resolve(sym).to_string(),
            })?;
        in_progress.push(sym);
        let v = eval_expr(expr, by_name, resolved, in_progress, it)?;
        in_progress.pop();
        resolved.insert(sym, v);
        Ok(v)
    }

    fn eval_expr(
        e: &ConstExpr<SymId>,
        by_name: &HashMap<SymId, &ConstExpr<SymId>>,
        resolved: &mut HashMap<SymId, f64>,
        in_progress: &mut Vec<SymId>,
        it: &Interner,
    ) -> Result<f64, ConstResolveError> {
        Ok(match e {
            ConstExpr::Lit(n) => *n,
            ConstExpr::Pi => std::f64::consts::PI,
            ConstExpr::Exp(arg) => eval_expr(arg, by_name, resolved, in_progress, it)?.exp(),
            ConstExpr::Neg(arg) => -eval_expr(arg, by_name, resolved, in_progress, it)?,
            ConstExpr::Bin(op, l, r) => {
                let lv = eval_expr(l, by_name, resolved, in_progress, it)?;
                let rv = eval_expr(r, by_name, resolved, in_progress, it)?;
                match op {
                    BinOp::Add => lv + rv,
                    BinOp::Sub => lv - rv,
                    BinOp::Mul => lv * rv,
                    BinOp::Div => {
                        if rv == 0.0 {
                            return Err(ConstResolveError::DivisionByZero);
                        }
                        lv / rv
                    }
                }
            }
            ConstExpr::Ref(name) => eval(*name, by_name, resolved, in_progress, it)?,
        })
    }

    // Drive evaluation for each declared const so that even consts
    // unreferenced in any value position are validated for cyclicity
    // and well-formedness.
    for d in decls {
        eval(d.name, &by_name, &mut resolved, &mut in_progress, it)?;
    }

    Ok(resolved)
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

    for n in desc.header.iter_mut() {
        substitute_in_node(n, &table, it)?;
    }
    for item in desc.items.iter_mut() {
        substitute_in_item(item, &table, it)?;
    }
    Ok(())
}

fn substitute_in_item(
    item: &mut HyperItem<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    match item {
        HyperItem::Node(n) => substitute_in_node(n, table, it),
        HyperItem::Edge(e) => substitute_in_edge(e, table, it),
        HyperItem::Arc(a) => {
            substitute_in_arc(&mut a.inner, table, it)?;
            substitute_in_anno(&mut a.anno, table, it)
        }
    }
}

fn substitute_in_node(
    n: &mut NodeDecl<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    substitute_in_anno(&mut n.anno, table, it)?;
    substitute_in_node_inner(&mut n.inner, table, it)
}

fn substitute_in_node_inner(
    inner: &mut NodeInner<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    for sr in inner.bases.iter_mut() {
        substitute_in_signed_ref(sr, table, it)?;
    }
    if let Some(body) = inner.body.as_mut() {
        for c in body.iter_mut() {
            substitute_in_item(c, table, it)?;
        }
    }
    Ok(())
}

fn substitute_in_edge(
    e: &mut EdgeDecl<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    substitute_in_anno(&mut e.anno, table, it)?;
    substitute_in_edge_inner(&mut e.inner, table, it)
}

fn substitute_in_edge_inner(
    inner: &mut EdgeInner<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    for sr in inner.bases.iter_mut() {
        substitute_in_signed_ref(sr, table, it)?;
    }
    for c in inner.body.iter_mut() {
        substitute_in_item(c, table, it)?;
    }
    Ok(())
}

fn substitute_in_arc(
    inner: &mut ArcInner<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    for sr in inner.refs.iter_mut() {
        substitute_in_signed_ref(sr, table, it)?;
    }
    Ok(())
}

fn substitute_in_signed_ref(
    sr: &mut SignedRef<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    let atom: &mut RefAtom<'_, SymId> = match sr {
        SignedRef::Plus(a) | SignedRef::Minus(a) | SignedRef::Neutral(a) => a,
    };
    substitute_in_anno(&mut atom.anno, table, it)
}

fn substitute_in_anno(
    a: &mut Anno<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    if let Some(v) = a.value.as_mut() {
        substitute_in_value(v, table, it)?;
    }
    Ok(())
}

fn substitute_in_value(
    v: &mut Value<'_, SymId>,
    table: &HashMap<SymId, f64>,
    it: &Interner,
) -> Result<(), ConstResolveError> {
    match v {
        Value::Str(_) | Value::Num(_) => Ok(()),
        Value::List(xs) => {
            for x in xs.iter_mut() {
                substitute_in_value(x, table, it)?;
            }
            Ok(())
        }
        Value::Ref(r) => {
            // Single-segment refs whose ident matches a const become
            // numeric literals; multi-segment refs are real cross-decl
            // references and the standard resolver handles them.
            if r.path.len() == 1 {
                let sym = r.path[0];
                if let Some(&f) = table.get(&sym) {
                    *v = Value::Num(f);
                }
            }
            Ok(())
        }
        Value::Expr(_) => {
            // Move out via mem::take so we can transform.
            let owned = std::mem::replace(
                v,
                Value::Expr(ConstExpr::Lit(0.0)), // placeholder, immediately overwritten
            );
            let expr = match owned {
                Value::Expr(e) => e,
                _ => unreachable!(),
            };
            // Re-use evaluate_consts machinery via a lightweight wrap:
            // expression doesn't reference anything outside the table,
            // so an empty by_name is fine.
            let by_name: HashMap<SymId, &ConstExpr<SymId>> = HashMap::new();
            let mut resolved_local: HashMap<SymId, f64> = table.clone();
            let mut in_progress: Vec<SymId> = Vec::new();
            let value =
                eval_in_context(&expr, &by_name, &mut resolved_local, &mut in_progress, it)?;
            *v = Value::Num(value);
            Ok(())
        }
    }
}

/// Stand-alone expression evaluator for use after the const table is
/// fully built — `resolved` already contains all const values, so we
/// only need to walk through `Lit`/`Bin`/`Neg`/`Pi`/`Exp` and look up
/// `Ref`s in the resolved table.
fn eval_in_context(
    e: &ConstExpr<SymId>,
    _by_name: &HashMap<SymId, &ConstExpr<SymId>>,
    resolved: &mut HashMap<SymId, f64>,
    _in_progress: &mut Vec<SymId>,
    it: &Interner,
) -> Result<f64, ConstResolveError> {
    Ok(match e {
        ConstExpr::Lit(n) => *n,
        ConstExpr::Pi => std::f64::consts::PI,
        ConstExpr::Exp(arg) => eval_in_context(arg, _by_name, resolved, _in_progress, it)?.exp(),
        ConstExpr::Neg(arg) => -eval_in_context(arg, _by_name, resolved, _in_progress, it)?,
        ConstExpr::Bin(op, l, r) => {
            let lv = eval_in_context(l, _by_name, resolved, _in_progress, it)?;
            let rv = eval_in_context(r, _by_name, resolved, _in_progress, it)?;
            match op {
                BinOp::Add => lv + rv,
                BinOp::Sub => lv - rv,
                BinOp::Mul => lv * rv,
                BinOp::Div => {
                    if rv == 0.0 {
                        return Err(ConstResolveError::DivisionByZero);
                    }
                    lv / rv
                }
            }
        }
        ConstExpr::Ref(name) => {
            *resolved
                .get(name)
                .ok_or_else(|| ConstResolveError::UndefinedRef {
                    name: it.resolve(*name).to_string(),
                })?
        }
    })
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
}
