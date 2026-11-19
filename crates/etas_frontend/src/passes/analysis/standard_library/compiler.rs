use etas_hir::{HirItem, HirType, HirTypeDeclBody, Visibility};
use etas_types::{Type, TypeId, TypeOutput};

/// The compiler service accepts either retained definition family. Source
/// supplies the generic spelling and complete public descriptor declarations.
pub(super) fn action_signature_matches(
    types: &TypeOutput,
    native: &etas_std::EffectActionDecl,
    source: &etas_types::EffectActionSignature,
) -> Option<bool> {
    if native.owner != "Code" || !matches!(native.name.as_str(), "check" | "compile") {
        return None;
    }
    let [parameter] = source.generic_params.as_slice() else {
        return Some(false);
    };
    let [input] = source.params.as_slice() else {
        return Some(false);
    };
    let Some(Type::Applied { constructor, args }) = types.store.get(*input) else {
        return Some(false);
    };
    let known = &types.facts.known_std_types.code;
    Some(
        parameter.kind == etas_types::CallableGenericParamKind::Type
            && parameter.bounds.is_empty()
            && [known.agent_definition, known.callable_definition]
                .contains(&Some(TypeId(constructor.0)))
            && args.as_slice() == [parameter.subject]
            && if native.name == "compile" {
                source.output == parameter.subject
            } else {
                matches!(types.store.get(source.output), Some(Type::Nominal(report))
                if report.name == "std.compiler.CheckReport" && report.params.is_empty())
            },
    )
}

pub(super) fn signature_matches(
    types: &TypeOutput,
    item: etas_hir::HirItemId,
    role: etas_std::StdExecutionRole,
) -> bool {
    let Some(etas_types::ItemSignature::Flow(signature)) = types.facts.item_signatures.get(&item)
    else {
        return false;
    };
    if signature
        .effects
        .as_ref()
        .is_none_or(|row| !row.effects.is_empty() || row.tail.is_some())
        || signature
            .requested_actions
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty() || row.tail.is_some())
        || !signature.generic_params.is_empty()
    {
        return false;
    }
    let known = &types.facts.known_std_types;
    if role == etas_std::StdExecutionRole::ParseBody {
        let Some((syntax, error)) = etas_types::value_representation::result_parameters(
            &types.store,
            known,
            signature.output,
        ) else {
            return false;
        };
        return matches!(signature.params.as_slice(), [text]
            if matches!(types.store.get(*text), Some(Type::Primitive(etas_types::PrimitiveType::String))))
            && known.code.syntax_block == Some(syntax)
            && matches!(types.store.get(error), Some(Type::Nominal(value)) if value.name == "std.compiler.ParseError");
    }
    let Some(method) = types
        .facts
        .type_impl_methods
        .values()
        .find(|method| method.body == item)
    else {
        return false;
    };
    let owner = match role {
        etas_std::StdExecutionRole::AgentWithBody => known.code.agent_definition,
        etas_std::StdExecutionRole::CallableWithBody => known.code.callable_definition,
        _ => return false,
    };
    let Some(syntax) = known.code.syntax_block else {
        return false;
    };
    method.inherent
        && method.owner_params.len() == 1
        && method.owner_params[0].kind == etas_types::CallableGenericParamKind::Type
        && matches!(types.store.get(method.self_type), Some(Type::Applied { constructor, args })
            if owner == Some(TypeId(constructor.0)) && args.as_slice() == [method.owner_params[0].subject])
        && signature.params.as_slice() == [method.self_type, syntax]
        && signature.output == method.self_type
}

pub(super) fn syntax_layout_diagnostics(
    hir: &etas_hir::HirProgram,
    types: &TypeOutput,
) -> Vec<etas_core::Diagnostic> {
    if !hir.source_standard_types {
        return Vec::new();
    }
    let Some(syntax) = types.facts.known_std_types.code.syntax_block else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    for (_, item) in hir.items.iter() {
        let HirItem::Type(source) = item else {
            continue;
        };
        if !matches!(types.facts.symbol_types.get(&source.symbol),
            Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) if constructor.0 == syntax.0)
        {
            continue;
        }
        let field = |owner, name: &str| {
            etas_types::record_fields_with_applied_params(&types.store, owner)
                .ok()
                .flatten()?
                .fields
                .into_iter()
                .find(|field| field.name == name)
                .map(|field| field.ty)
        };
        let private = |name: &str| {
            matches!(&source.body, HirTypeDeclBody::Representation(body)
            if matches!(&hir.types[*body], HirType::Record { fields, .. }
                if fields.iter().any(|field| field.visibility == etas_syntax::ast::Visibility::Private
                    && hir.symbols.get(field.symbol).is_some_and(|symbol| symbol.name == name))))
        };
        let retained = field(syntax, "retained");
        let opaque = retained.is_some_and(|ty| {
            matches!(types.store.get(ty), Some(Type::Nominal(value)) if value.params.is_empty() && value.representation.is_none())
                && hir.symbols.iter().any(|symbol| symbol.visibility == Visibility::Private
                    && matches!(types.facts.symbol_types.get(&symbol.id), Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) if constructor.0 == ty.0))
        });
        let tokens = field(syntax, "tokens").and_then(|ty| match types.store.get(ty) {
            Some(Type::Array(element)) => Some(*element),
            _ => None,
        });
        let token_shape = tokens.is_some_and(|ty| {
            let text = field(ty, "text").is_some_and(|ty| {
                matches!(
                    types.store.get(ty),
                    Some(Type::Primitive(etas_types::PrimitiveType::String))
                )
            });
            let kind = field(ty, "kind").and_then(|ty| types.facts.enum_layouts.get(&ty));
            text && kind.is_some_and(|kind| {
                [
                    "Identifier",
                    "Keyword",
                    "Punctuation",
                    "Other",
                    "Trivia",
                    "Eof",
                ]
                .iter()
                .all(|name| {
                    kind.variants
                        .iter()
                        .any(|variant| variant.name == *name && variant.fields.is_empty())
                })
            })
        });
        let text = field(syntax, "text").is_some_and(|ty| {
            matches!(
                types.store.get(ty),
                Some(Type::Primitive(etas_types::PrimitiveType::String))
            )
        });
        let maximum = field(syntax, "format_limit").is_some_and(|ty| {
            matches!(
                types.store.get(ty),
                Some(Type::Primitive(etas_types::PrimitiveType::USize))
            )
        });
        if !source.type_params.is_empty()
            || !opaque
            || !private("retained")
            || !private("tokens")
            || !private("format_limit")
            || !token_shape
            || !text
            || !maximum
        {
            diagnostics.push(etas_core::Diagnostic::type_check(etas_core::TypeDiagnosticCode::TypeMismatch, source.span,
                "source SyntaxBlock requires its private opaque AST, private token observations and format ceiling, and canonical text projection"));
        }
    }
    diagnostics
}

/// Only the live capability crossing a compiler operation is constrained here.
/// Public descriptions and algorithms are checked as ordinary source bodies.
pub(super) fn type_layout_diagnostics(
    hir: &etas_hir::HirProgram,
    types: &TypeOutput,
) -> Vec<etas_core::Diagnostic> {
    if !hir.source_standard_types {
        return Vec::new();
    }
    let known = &types.facts.known_std_types.code;
    let mut retained = None;
    let mut diagnostics = Vec::new();
    for (_, item) in hir.items.iter() {
        if let HirItem::Enum(source) = item
            && let Some(etas_types::SymbolTypeFact::Type { constructor }) =
                types.facts.symbol_types.get(&source.symbol)
            && types.facts.known_std_types.option_enum == Some(TypeId(constructor.0))
        {
            let owner = TypeId(constructor.0);
            let parameters = types.facts.type_generics.get(&owner).map(Vec::as_slice);
            let layout = types.facts.enum_layouts.get(&owner);
            let valid = match (parameters, layout) {
                (Some([parameter]), Some(layout))
                    if parameter.kind == etas_types::CallableGenericParamKind::Type =>
                {
                    layout.variants.iter().any(|variant| {
                        variant.name == "Some"
                            && variant.field_names.is_none()
                            && variant.fields.as_slice() == [parameter.subject]
                    }) && layout
                        .variants
                        .iter()
                        .any(|variant| variant.name == "None" && variant.fields.is_empty())
                }
                _ => false,
            };
            if !valid {
                diagnostics.push(etas_core::Diagnostic::type_check(
                    etas_core::TypeDiagnosticCode::TypeMismatch, source.span,
                    "compiler optional data requires source Option<T> with Some(T) and None constructors",
                ));
            }
        }
        let HirItem::Type(source) = item else {
            continue;
        };
        let Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) =
            types.facts.symbol_types.get(&source.symbol)
        else {
            continue;
        };
        let owner = TypeId(constructor.0);
        if known.agent_definition != Some(owner) && known.callable_definition != Some(owner) {
            continue;
        }
        let capability = match &source.body {
            HirTypeDeclBody::Representation(body) => match &hir.types[*body] {
                HirType::Record { fields, .. } => fields
                    .iter()
                    .find(|field| {
                        hir.symbols
                            .get(field.symbol)
                            .is_some_and(|symbol| symbol.name == "retained")
                            && field.visibility == etas_syntax::ast::Visibility::Private
                    })
                    .and_then(|_| {
                        let Some(Type::Nominal(owner)) = types.store.get(owner) else {
                            return None;
                        };
                        let Some(Type::Record(fields)) =
                            owner.representation.and_then(|ty| types.store.get(ty))
                        else {
                            return None;
                        };
                        fields
                            .fields
                            .iter()
                            .find(|field| field.name == "retained")
                            .map(|field| field.ty)
                    }),
                _ => None,
            },
            _ => None,
        };
        let private_opaque = capability.is_some_and(|ty| {
            matches!(types.store.get(ty), Some(Type::Nominal(value))
                if value.params.is_empty() && value.representation.is_none())
                && hir.symbols.iter().any(|symbol| {
                    symbol.visibility == Visibility::Private
                        && matches!(types.facts.symbol_types.get(&symbol.id),
                            Some(etas_types::SymbolTypeFact::NominalType { constructor, .. }) if constructor.0 == ty.0)
                })
        });
        if !private_opaque || retained.is_some_and(|previous| capability != Some(previous)) {
            diagnostics.push(etas_core::Diagnostic::type_check(
                etas_core::TypeDiagnosticCode::TypeMismatch, source.span,
                "compiler definition records require the same private opaque retained capability; public descriptions cannot authorize a compiler operation",
            ));
        }
        retained = retained.or(capability);
    }
    diagnostics
}
