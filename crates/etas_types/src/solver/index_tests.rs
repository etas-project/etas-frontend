use super::*;
use etas_core::{SourceId, Span, TextRange, TypeDiagnosticCode};

#[test]
fn index_shape_propagation_does_not_publish_checked_facts_or_accept_invalid_indices() {
    let mut types = crate::TypeInterner::new();
    types.primitive(PrimitiveType::U8);
    let base = types.primitive(PrimitiveType::Bytes);
    let output = types.intern(Type::Var(crate::TypeVarId(0)));
    let index = types.primitive(PrimitiveType::Bool);
    let substitutions = Substitution::default();
    let specs = SpecFacts::default();
    let input = index::IndexAccessSolveInput {
        spec_facts: &specs,
        expr: etas_hir::HirExprId(0),
        base,
        index,
        output,
        index_error: None,
        origin: ConstraintOrigin {
            span: Span::new(SourceId(7), TextRange::default()),
        },
        substitutions: &substitutions,
    };
    let inferred = index::infer_index_access(types.store(), &input);
    assert!(inferred.failures.is_empty());
    assert!(inferred.substitutions.iter().next().is_some());
    assert!(inferred.index_facts.is_empty());
    assert!(inferred.checked_index_errors.is_empty());
    let checked = index::solve_index_access(types.store(), input);
    assert_eq!(checked.failures.len(), 1);
    assert!(checked.index_facts.is_empty());
    assert!(checked.checked_index_errors.is_empty());
}

#[test]
fn index_shape_propagation_reuses_key_bindings_when_checking_the_result() {
    let mut types = crate::TypeInterner::new();
    let shared = types.intern(Type::Var(crate::TypeVarId(0)));
    let base = types.intern(Type::Map {
        key: shared,
        value: shared,
    });
    let index = types.primitive(PrimitiveType::U8);
    let output = types.primitive(PrimitiveType::I32);
    let substitutions = Substitution::default();
    let specs = SpecFacts::default();
    let inferred = index::infer_index_access(
        types.store(),
        &index::IndexAccessSolveInput {
            spec_facts: &specs,
            expr: etas_hir::HirExprId(0),
            base,
            index,
            output,
            index_error: None,
            origin: ConstraintOrigin {
                span: Span::new(SourceId(7), TextRange::default()),
            },
            substitutions: &substitutions,
        },
    );
    assert_eq!(inferred.failures.len(), 1);
    assert!(inferred.substitutions.iter().next().is_none());
    assert!(inferred.index_facts.is_empty());
}

#[test]
fn index_shape_propagation_rejects_missing_primitive_facts() {
    let mut types = crate::TypeInterner::new();
    let base = types.primitive(PrimitiveType::Bytes);
    let output = types.intern(Type::Var(crate::TypeVarId(0)));
    let index = types.primitive(PrimitiveType::I32);
    let substitutions = Substitution::default();
    let specs = SpecFacts::default();
    let inferred = index::infer_index_access(
        types.store(),
        &index::IndexAccessSolveInput {
            spec_facts: &specs,
            expr: etas_hir::HirExprId(0),
            base,
            index,
            output,
            index_error: None,
            origin: ConstraintOrigin {
                span: Span::new(SourceId(7), TextRange::default()),
            },
            substitutions: &substitutions,
        },
    );
    assert_eq!(
        inferred.failures[0].code,
        TypeDiagnosticCode::IncompleteTypeFacts
    );
    assert!(inferred.substitutions.iter().next().is_none());
    assert!(inferred.index_facts.is_empty());
}
