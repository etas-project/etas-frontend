use etas_hir::{HirSourcePreludeBinding, HirSourcePreludeTarget};
use etas_std::StdNamespace;

use super::common::parsed_by_source;
use crate::{AstItemKind, ImportTarget, ProjectContext};

pub(super) fn bindings(context: &ProjectContext) -> Result<Vec<HirSourcePreludeBinding>, String> {
    let modules = context.modules.as_ref().expect("source module index");
    let imports = context
        .resolved_imports
        .as_ref()
        .expect("resolved source imports");
    let catalog = context
        .module_catalog
        .as_ref()
        .expect("source module catalog");
    let parsed = parsed_by_source(context);
    let mut result = Vec::new();
    let Some((prelude, module)) = modules
        .modules
        .iter()
        .find(|(_, module)| module.path.segments == ["std", "prelude"])
    else {
        return Ok(result);
    };
    let declaration = |item: &crate::AstItemRef, member: Option<String>| {
        let source = parsed
            .get(&item.source)
            .expect("source prelude declaration syntax");
        let span = source.parse.value.items[item.index].item.span();
        HirSourcePreludeTarget::Declaration { span, member }
    };
    let target = |name: String, target: &ImportTarget| -> Result<HirSourcePreludeBinding, String> {
        let (namespace, target) = match target {
            ImportTarget::SourceItem { item, member, .. } => {
                let namespace = if member.is_some() {
                    if item.kind == AstItemKind::Effect {
                        StdNamespace::Effect
                    } else {
                        StdNamespace::Value
                    }
                } else {
                    namespace(item.kind)?
                };
                (namespace, declaration(item, member.clone()))
            }
            ImportTarget::Module(crate::ResolvedModuleTarget::Source { .. }) => (
                StdNamespace::Module,
                HirSourcePreludeTarget::Module {
                    path: target
                        .catalog_path(catalog)
                        .expect("source module import path"),
                },
            ),
            ImportTarget::StdItem { symbol, .. } => {
                let symbol = context
                    .std_registry
                    .symbol(*symbol)
                    .expect("compiler primitive import");
                if !matches!(&symbol.decl, etas_std::StdDecl::Type(declaration) if declaration.kind == etas_std::TypeDeclKind::Primitive)
                {
                    return Err(format!(
                        "source prelude `{name}` has no source-owned declaration"
                    ));
                }
                (
                    symbol.kind.namespace(),
                    HirSourcePreludeTarget::Primitive { symbol: symbol.id },
                )
            }
            _ => {
                return Err(format!(
                    "source prelude `{name}` must refer to the std source package or a language primitive"
                ));
            }
        };
        Ok(HirSourcePreludeBinding {
            name,
            namespace,
            target,
        })
    };
    for (name, export) in &module.visibility_exports.items {
        // Resolved re-exports are handled below so contextual names are not
        // flattened by the module's ordinary single-name export view.
        if module
            .parts
            .iter()
            .any(|part| modules.parts[*part].source == export.item.source)
        {
            result.push(HirSourcePreludeBinding {
                name: name.clone(),
                namespace: namespace(export.item.kind)?,
                target: declaration(&export.item, None),
            });
        }
    }
    for import in &imports.imports {
        if import.from == prelude && import.visibility == etas_hir::Visibility::Public {
            result.push(target(import.local_name.clone(), &import.target)?);
        }
    }
    for import in &imports.wildcard_imports {
        if import.from == prelude && import.visibility == etas_hir::Visibility::Public {
            for (name, item) in &import.targets {
                result.push(target(name.clone(), item)?);
            }
        }
    }
    Ok(result)
}

fn namespace(kind: AstItemKind) -> Result<StdNamespace, String> {
    Ok(match kind {
        AstItemKind::TypeAlias
        | AstItemKind::Type
        | AstItemKind::Enum
        | AstItemKind::Spec
        | AstItemKind::Agent
        | AstItemKind::Protocol => StdNamespace::Type,
        AstItemKind::Effect => StdNamespace::Effect,
        AstItemKind::Flow | AstItemKind::Tool | AstItemKind::TopLevelLet => StdNamespace::Value,
        AstItemKind::Impl | AstItemKind::Error => {
            return Err("source prelude export has no declaration namespace".into());
        }
    })
}
