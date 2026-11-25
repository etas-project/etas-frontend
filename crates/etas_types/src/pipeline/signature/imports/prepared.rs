use super::external::{self, ExternalBindingState};
use crate::{ExternalPackageKey, ExternalSignatureInput, TypeFacts, TypeInterner, TypeStore};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct PreparedExternalSignatures {
    pub package: ExternalPackageKey,
    base: crate::ty::SharedTypeBase,
    facts: TypeFacts,
    bindings: ExternalBindingState,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    paths: HashMap<Vec<String>, crate::TypeId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    aliases: HashMap<Vec<String>, (Vec<String>, crate::TypeId)>,
}

pub fn prepare(
    registry: Arc<etas_std::StdRegistry>,
    metadata: crate::ExternalPublicMetadataInput,
    reflection: crate::api::ReflectionSignatureInput,
) -> Result<PreparedExternalSignatures, String> {
    let mut program = etas_hir::HirProgram::default();
    program.source_standard_types = true;
    let mut ctx =
        crate::pipeline::context::TypePipelineContext::new_with_std_registry(&program, registry);
    super::reflection::initialize(&mut ctx, &reflection);
    super::std::apply_std_signature_input(&mut ctx, &Default::default());
    let package = metadata.package;
    let span = etas_core::Span::new(etas_core::SourceId(0), Default::default());
    let input = ExternalSignatureInput {
        package_anchors: vec![(package, span)],
        metadata: vec![metadata],
        ..Default::default()
    };
    let bindings = external::prepare_external_signature_input(&mut ctx, &input)
        .ok_or("cannot prepare imported declarations")?;
    external::apply_external_spec_facts(&mut ctx, &input);
    bind_standard_identities(&mut ctx, package);
    super::reflection::apply(&mut ctx, &reflection);
    if let Some(error) = ctx
        .diagnostics
        .iter()
        .find(|d| d.severity == etas_core::Severity::Error)
    {
        return Err(error.message.clone());
    }
    Ok(PreparedExternalSignatures {
        package,
        base: crate::ty::SharedTypeBase::new(ctx.interner.into_store()),
        facts: ctx.signature_facts,
        bindings,
        paths: ctx.external_type_paths,
        aliases: ctx.external_type_aliases,
    })
}

impl PreparedExternalSignatures {
    pub(crate) fn initialize(
        &self,
        ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
        input: &ExternalSignatureInput,
    ) {
        ctx.interner = TypeInterner::from_store(TypeStore::with_base(self.base.clone()));
        ctx.signature_facts = self.facts.clone();
        let anchor = input
            .package_anchors
            .iter()
            .find(|(id, _)| *id == self.package)
            .expect("prepared package anchor")
            .1;
        for method in &mut ctx.signature_facts.external_methods {
            method.span = anchor;
        }
        ctx.external_type_paths = self.paths.clone();
        ctx.external_type_aliases = self.aliases.clone();
    }
    pub(crate) fn bind(
        &self,
        ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
        input: &ExternalSignatureInput,
    ) {
        // Prelude binding runs against this caller's symbols. Keep the SDK's
        // canonical source signatures rather than its registry bootstrap views.
        ctx.signature_facts
            .library_callable_signatures
            .extend(self.facts.library_callable_signatures.clone());
        ctx.signature_facts
            .std_callable_signatures
            .extend(self.facts.std_callable_signatures.clone());
        external::bind_external_signature_input(ctx, input, &self.bindings);
    }
}

pub(crate) fn bind_standard_identities(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    package: ExternalPackageKey,
) {
    // The installing compiler can prepare signatures with only bootstrap
    // contracts. The consuming SDK registry owns the ordinary std identities.
    let declarations = ctx
        .signature_facts
        .external_specs
        .signatures
        .keys()
        .filter(|identity| identity.package == package.0)
        .cloned()
        .collect::<Vec<_>>();
    for external in declarations {
        let Some(symbol) = ctx.std_registry.lookup_qualified(&external.path) else {
            continue;
        };
        let Some(identity) = ctx.std_registry.declaration_key(symbol.id) else {
            continue;
        };
        if ctx
            .signature_facts
            .reflection_declaration_identities
            .get(&external.path)
            == Some(identity)
        {
            ctx.signature_facts
                .external_specs
                .std_identities
                .insert(external, identity.clone());
        }
    }
}
