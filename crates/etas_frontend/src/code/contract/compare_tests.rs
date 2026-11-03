use super::*;
use crate::{EntryPolicy, Frontend, ProjectInput, SourceInput};
use std::sync::Arc;

#[test]
fn missing_public_or_latent_facts_are_not_empty_residuals() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; public flow make() -> () -> unit ![] { return () => { return; }; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let original = output.checked.unwrap();
    let owner = original
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if original.symbols.get(flow.symbol).unwrap().name == "make" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let root = &original.effects.item_effects[&owner];
    assert_eq!(
        residual_requirements(&original, owner, Some(root), true, false).unwrap(),
        Some(BTreeSet::new())
    );
    for remove_contract in [false, true] {
        let mut checked = original.clone();
        let effects = Arc::make_mut(&mut Arc::make_mut(&mut checked).effects);
        let index = effects
            .public_contracts
            .iter()
            .position(|contract| contract.item == owner)
            .unwrap();
        if remove_contract {
            effects.public_contracts.remove(index);
        } else {
            let value = effects.public_contracts[index].latent_flows[0].value;
            effects.latent_effects.remove(&value);
        }
        let root = &checked.effects.item_effects[&owner];
        assert_eq!(
            residual_requirements(&checked, owner, Some(root), true, false).unwrap(),
            None
        );
    }
}

#[test]
fn reachable_proof_requires_solved_lambda_facts_and_preserves_obligations() {
    use etas_code::{
        CodeFingerprint,
        residual::identity::{ObligationId, ResidualObligationRef},
    };
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; let callback: (i32) -> i32 = (value: i32) => { return value; };
         flow original() -> (i32) -> i32 { return callback; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let original = output.checked.unwrap();
    let owner = original
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if original.symbols.get(flow.symbol).unwrap().name == "original" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        reachable_residual_requirements(&original, owner),
        Some(BTreeSet::new())
    );
    let value = *original.effects.latent_effects.keys().next().unwrap();
    let reference = ResidualObligationRef::new(
        ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([55; 32])),
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        residual_relation(
            Some(BTreeSet::new()),
            Some(BTreeSet::from([reference.clone()])),
            &original,
            &original,
            None,
        ),
        R::Unproven(Unknown::UnsupportedResidualChecks),
        "a descriptive obligation without a matching verified plan cannot authorize guarded emission"
    );
    let mut with_residual = original.clone();
    Arc::make_mut(&mut Arc::make_mut(&mut with_residual).effects)
        .latent_effects
        .get_mut(&value)
        .unwrap()
        .summary
        .residual_checks
        .insert(reference);
    assert_eq!(
        reachable_residual_requirements(&with_residual, owner).map(|refs| refs.len()),
        Some(1)
    );
    let mut missing = original.clone();
    Arc::make_mut(&mut Arc::make_mut(&mut missing).effects)
        .latent_effects
        .remove(&value);
    assert_eq!(reachable_residual_requirements(&missing, owner), None);
}

#[test]
fn preserved_generic_output_requires_reachable_residual_evidence() {
    use etas_code::{
        CodeFingerprint,
        residual::identity::{ObligationId, ResidualObligationRef},
    };
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; flow original<T>(value: T) -> T {
            let callback = (number: i32) => { return number; };
            return value;
        }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let original = output.checked.unwrap();
    let owner = original
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(flow)
                if original.symbols.get(flow.symbol).unwrap().name == "original" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let root = &original.effects.item_effects[&owner];
    assert_eq!(
        residual_requirements(&original, owner, Some(root), true, true).unwrap(),
        Some(BTreeSet::new())
    );
    let value = *original.effects.latent_effects.keys().next().unwrap();
    let mut with_residual = original.clone();
    Arc::make_mut(&mut Arc::make_mut(&mut with_residual).effects)
        .latent_effects
        .get_mut(&value)
        .unwrap()
        .summary
        .residual_checks
        .insert(
            ResidualObligationRef::new(
                ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([56; 32])),
                Default::default(),
            )
            .unwrap(),
        );
    let root = &with_residual.effects.item_effects[&owner];
    assert_eq!(
        residual_requirements(&with_residual, owner, Some(root), true, true)
            .unwrap()
            .map(|refs| refs.len()),
        Some(1)
    );
    let mut missing = original.clone();
    Arc::make_mut(&mut Arc::make_mut(&mut missing).effects)
        .latent_effects
        .remove(&value);
    let root = &missing.effects.item_effects[&owner];
    assert_eq!(
        residual_requirements(&missing, owner, Some(root), true, true).unwrap(),
        None
    );
}
