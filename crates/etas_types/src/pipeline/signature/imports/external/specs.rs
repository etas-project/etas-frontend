use super::*;
use crate::facts::specs::{ExternalSpecIdentity, ExternalSpecSignatureFact};

pub(super) fn prepare(ctx: &mut TypePipelineContext<'_>, input: &ExternalSignatureInput) {
    for metadata in &input.metadata {
        if package_binding_span(input, metadata.package).is_none() {
            continue;
        }
        for signature in &metadata.spec_signatures {
            let identity = ExternalSpecIdentity {
                package: metadata.package.0,
                path: signature.path.clone(),
            };
            ctx.signature_facts.external_specs.signatures.insert(
                identity,
                ExternalSpecSignatureFact {
                    kind: match signature.kind {
                        crate::ExternalSpecKindInput::Type => SpecKind::TypeSpec,
                        crate::ExternalSpecKindInput::Callable => SpecKind::CallableSpec,
                        crate::ExternalSpecKindInput::Trace => SpecKind::TraceSpec,
                    },
                    param_names: signature.param_names.clone(),
                    super_specs: Vec::new(),
                    methods: Vec::new(),
                },
            );
        }
    }
    for metadata in &input.metadata {
        let Some(span) = package_binding_span(input, metadata.package) else {
            continue;
        };
        for signature in &metadata.spec_signatures {
            let identity = ExternalSpecIdentity {
                package: metadata.package.0,
                path: signature.path.clone(),
            };
            let mut supers = Vec::new();
            for bound in &signature.super_specs {
                let spec = match resolve_external_spec(
                    ctx,
                    metadata.package,
                    &bound.spec,
                    bound.args.len(),
                ) {
                    Ok(spec) => spec,
                    Err(reason) => {
                        incomplete_external_type_facts(ctx, span, reason);
                        continue;
                    }
                };
                let args = bound
                    .args
                    .iter()
                    .map(|arg| lower_external_type(ctx, span, arg))
                    .collect();
                supers.push(crate::CheckedSpecBound { spec, args });
            }
            let methods = lower_external_spec_methods(ctx, metadata.package, span, signature);
            if let Some(fact) = ctx
                .signature_facts
                .external_specs
                .signatures
                .get_mut(&identity)
            {
                fact.super_specs = supers;
                fact.methods = methods;
            }
        }
    }
    for binding in &input.symbol_bindings {
        let identity = ExternalSpecIdentity {
            package: binding.package.0,
            path: binding.path.clone(),
        };
        if ctx
            .signature_facts
            .external_specs
            .signatures
            .contains_key(&identity)
        {
            ctx.signature_facts
                .external_specs
                .aliases
                .insert(binding.symbol, identity);
        }
    }
}
