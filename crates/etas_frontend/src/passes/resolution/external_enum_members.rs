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
