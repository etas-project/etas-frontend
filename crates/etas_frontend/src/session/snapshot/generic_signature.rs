use super::*;

pub(super) fn parts(parameters: &[SymbolId], hir: &HirProgram) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    for (index, parameter) in parameters.iter().enumerate() {
        let Some(symbol) = hir.symbols.get(*parameter) else {
            return Err(format!(
                "generic signature has no parameter symbol {}",
                parameter.0
            ));
        };
        match &symbol.def {
            SymbolDef::EffectParam { .. } => {
                parts.push(format!("generic:{index}:effect:{}", symbol.name))
            }
            SymbolDef::TypeParam { bounds, .. } => {
                parts.push(format!("generic:{index}:type:{}", symbol.name));
                for (bound_index, bound) in bounds.iter().enumerate() {
                    parts.push(format!(
                        "generic:{index}:bound:{bound_index}:{}",
                        path_signature_part(&bound.path)
                    ));
                    for (arg_index, argument) in bound.args.iter().enumerate() {
                        parts.push(format!(
                            "generic:{index}:bound:{bound_index}:arg:{arg_index}:{}",
                            hir_type_signature_part(*argument, hir)
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "generic signature references a non-parameter symbol {}",
                    parameter.0
                ));
            }
        }
    }
    Ok(parts)
}
