use crate::{ProjectContext, ProjectExternalTypeInput};
use etas_hir::{HirProgram, SymbolData, SymbolDef, SymbolKind};

pub(super) fn install(context: &mut ProjectContext, hir: &mut HirProgram) -> Result<(), String> {
    let environment = context
        .validated_external_environment
        .as_ref()
        .ok_or("missing validated environment for enum members")?
        .environment();
    let bindings = context
        .hir_item_bindings
        .as_mut()
        .ok_or("missing HIR bindings for enum members")?;
    let owners = bindings
        .external_import_members
        .iter()
        .map(|(symbol, binding)| (*symbol, binding.clone()))
        .collect::<Vec<_>>();
    for (owner, binding) in owners {
        let Some(declaration) = environment
            .external_public_metadata
            .iter()
            .filter(|metadata| metadata.package == binding.package)
            .flat_map(|metadata| &metadata.enums)
            .find(|declaration| declaration.path == binding.path)
        else {
            continue;
        };
        let Some(ProjectExternalTypeInput::Enum { path, variants }) = &declaration.ty else {
            return Err("external enum has no validated constructor layout".into());
        };
        if path != &binding.path {
            return Err("external enum layout disagrees with import identity".into());
        }
        let owner_data = hir
            .symbols
            .get(owner)
            .ok_or("external enum owner has no HIR symbol")?
            .clone();
        for (index, variant) in variants.iter().enumerate() {
            let index =
                u32::try_from(index).map_err(|_| "external enum has too many constructors")?;
            let symbol = hir.symbols.alloc(SymbolData {
                name: variant.name.clone(),
                kind: SymbolKind::EnumVariant,
                visibility: owner_data.visibility,
                defining_module: owner_data.defining_module,
                defining_item: None,
                def: SymbolDef::ExternalEnumVariant {
                    enum_path: path.clone(),
                    variant_index: index,
                },
                declared_type: None,
                definition_span: binding.span,
            });
            hir.symbols
                .insert_member(owner, variant.name.clone(), symbol);
            let mut member = binding.clone();
            member.path.push(variant.name.clone());
            bindings.external_import_members.insert(symbol, member);
        }
    }
    Ok(())
}

/// Standard enums use the selected registry's checked layout, independently of
/// external dependency metadata and the source program's own enum declarations.
pub(super) fn install_std(registry: &etas_std::StdRegistry, hir: &mut HirProgram) {
    // Lowering can resolve a qualified constructor before import members are
    // installed. Give those existing aliases the same layout provenance.
    let constructors = hir
        .symbols
        .iter()
        .filter_map(|symbol| {
            let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
                return None;
            };
            if hir.source_import_targets.contains_key(&symbol.id) {
                return None;
            }
            let variant = registry.lookup_qualified(path)?;
            let owner = variant.enum_owner?;
            let index = registry
                .enum_constructors(owner)
                .position(|candidate| candidate.id == variant.id)?;
            Some((
                symbol.id,
                index.try_into().expect("standard enum layout capacity"),
            ))
        })
        .collect::<Vec<_>>();
    for (id, variant_index) in constructors {
        let symbol = hir.symbols.get_mut(id).unwrap();
        symbol.kind = SymbolKind::EnumVariant;
        let SymbolDef::ImportAlias { origin, .. } = &mut symbol.def else {
            unreachable!()
        };
        *origin = etas_hir::ImportAliasOrigin::StdEnumVariant { variant_index };
    }
    let owners = hir.symbols.iter().filter_map(|owner| {
        let SymbolDef::ImportAlias { path, origin } = &owner.def else { return None; };
        if hir.source_import_targets.contains_key(&owner.id)
            || matches!(origin, etas_hir::ImportAliasOrigin::StdEnumVariant { .. }) { return None; }
        let declaration = registry.lookup_qualified(path)?;
        matches!(&declaration.decl, etas_std::StdDecl::Type(ty) if ty.kind == etas_std::TypeDeclKind::Enum)
            .then(|| (owner.clone(), declaration.id))
    }).collect::<Vec<_>>();
    for (owner, id) in owners {
        for (index, variant) in registry.enum_constructors(id).enumerate() {
            let symbol = hir.symbols.alloc(SymbolData {
                name: variant.name.clone(),
                kind: SymbolKind::EnumVariant,
                visibility: owner.visibility,
                defining_module: owner.defining_module,
                defining_item: None,
                def: SymbolDef::ImportAlias {
                    path: variant.qualified_path.clone(),
                    origin: etas_hir::ImportAliasOrigin::StdEnumVariant {
                        variant_index: index.try_into().expect("standard enum layout capacity"),
                    },
                },
                declared_type: None,
                definition_span: owner.definition_span,
            });
            hir.symbols
                .insert_member(owner.id, variant.name.clone(), symbol);
        }
    }
}
