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
        external::bind_external_signature_input(ctx, input, &self.bindings);
    }
}
