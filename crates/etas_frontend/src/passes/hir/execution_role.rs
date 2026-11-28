use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{
    HirAnnotationArg, HirExpr, HirFlowBody, HirItem, HirLiteral, HirProgram, Visibility,
};
use etas_std::StdExecutionRole;

pub(super) fn bind(hir: &mut HirProgram, mode: crate::StandardLibraryMode) {
    if mode != crate::StandardLibraryMode::SourceBootstrap {
        return;
    }
    let mut declarations = hir.item_annotations.iter().collect::<Vec<_>>();
    declarations.sort_by_key(|(item, _)| **item);
    let mut selected = Vec::new();
    for (item, annotations) in declarations {
        for annotation in annotations {
            if !annotation
                .path
                .segments
                .iter()
                .map(|part| part.name.as_str())
                .eq(["std", "role"])
            {
                continue;
            }
            let [HirAnnotationArg::Positional { value, .. }] = annotation.args.as_slice() else {
                continue;
            };
            let Some(HirExpr::Literal(HirLiteral::String { value, .. })) = hir.exprs.get(*value)
            else {
                continue;
            };
            let Some(role) = StdExecutionRole::from_role(value) else {
                continue;
            };
            selected.push((*item, role, annotation.span));
        }
    }
    for (item, role, span) in selected {
        let valid = match &hir.items[item] {
            HirItem::Flow(flow) => {
                let symbol = hir.symbols.get(flow.symbol).unwrap();
                matches!(flow.body, HirFlowBody::Decl { .. })
                    && flow.return_type.is_some()
                    && flow.effects.is_some()
                    && flow.receiver_mode
                        == if role.has_receiver() {
                            etas_hir::HirReceiverMode::Value
                        } else {
                            etas_hir::HirReceiverMode::None
                        }
                    && (symbol.visibility == Visibility::Public) == role.is_public()
                    && canonical_execution_owner(hir, item, flow, role)
            }
            _ => false,
        };
        if !valid {
            hir.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::TypeMismatch,
                span,
                format!("execution role `{}` requires its canonical flow declaration and visibility, explicit result and effect row", role.role()),
            ));
            continue;
        }
        let HirItem::Flow(flow) = hir.items.get_mut(item).unwrap() else {
            unreachable!()
        };
        flow.body = HirFlowBody::Runtime { role, span };
    }
}

fn canonical_execution_owner(
    hir: &HirProgram,
    item: etas_hir::HirItemId,
    flow: &etas_hir::HirFlowDecl,
    role: StdExecutionRole,
) -> bool {
    let owner = hir.items.iter().find_map(|(_, candidate)| {
        let HirItem::Impl(implementation) = candidate else {
            return None;
        };
        implementation.items.iter().any(|member| {
            matches!(member, etas_hir::HirImplItem::Callable { item: member } if *member == item)
        }).then_some(implementation)
    });
    let expected_owners = role.associated_owners();
    if expected_owners.is_empty() {
        return owner.is_none()
            && super::native_role::canonical_path(hir, flow.symbol).join(".")
                == role.source_path();
    }
    let Some(implementation) = owner else {
        return false;
    };
    let etas_hir::HirImplTarget::Inherent { target, type_args } = &implementation.target else {
        return false;
    };
    let etas_hir::ResolveResult::Resolved(symbol) = target.resolution else {
        return false;
    };
    let declaration = hir.symbols.get(symbol).unwrap();
    let method = hir.symbols.get(flow.symbol).unwrap();
    let shape = match declaration
        .defining_item
        .and_then(|item| hir.items.get(item))
    {
        Some(HirItem::Enum(owner)) => {
            matches!(
                role,
                StdExecutionRole::CaptureData
                    | StdExecutionRole::CaptureStorage
                    | StdExecutionRole::RestoreStorage
                    | StdExecutionRole::StorageSchema
            ) && owner.type_params.is_empty()
        }
        Some(HirItem::Type(owner)) => {
            owner.type_params.len() == role.owner_type_parameter_count()
                && match (&owner.body, role) {
                    (
                        etas_hir::HirTypeDeclBody::Bodyless,
                        StdExecutionRole::ComparatorCapture | StdExecutionRole::ComparatorSame,
                    ) => true,
                    (
                        etas_hir::HirTypeDeclBody::Representation(body),
                        StdExecutionRole::AgentWithBody | StdExecutionRole::CallableWithBody,
                    ) => {
                        matches!(&hir.types[*body], etas_hir::HirType::Record { fields, .. }
                            if fields.iter().any(|field| field.visibility == etas_syntax::ast::Visibility::Private
                                && hir.symbols.get(field.symbol).is_some_and(|symbol| symbol.name == "retained")))
                    }
                    (
                        etas_hir::HirTypeDeclBody::Representation(body),
                        StdExecutionRole::StreamClock
                        | StdExecutionRole::ReadScopeEnter
                        | StdExecutionRole::ScopeEnter
                        | StdExecutionRole::ResourceScope
                        | StdExecutionRole::ArrayTruncate
                        | StdExecutionRole::CachedMapIndex
                        | StdExecutionRole::RememberMapIndex
                        | StdExecutionRole::PayloadConvert,
                    ) => {
                        matches!(&hir.types[*body], etas_hir::HirType::Record { fields, .. }
                            if fields.is_empty())
                    }
                    (
                        etas_hir::HirTypeDeclBody::Representation(body),
                        StdExecutionRole::StorageContext
                        | StdExecutionRole::StorageNonce
                        | StdExecutionRole::CachedSchemaFingerprint
                        | StdExecutionRole::RememberSchemaFingerprint
                        | StdExecutionRole::Requeue
                        | StdExecutionRole::Park
                        | StdExecutionRole::TaskNextCompletion
                        | StdExecutionRole::TaskCancel
                        | StdExecutionRole::MessageContextCapture
                        | StdExecutionRole::MessageContextRecord,
                    ) => {
                        matches!(&hir.types[*body], etas_hir::HirType::Record { fields, .. }
                    if fields.iter().all(|field| field.visibility == etas_syntax::ast::Visibility::Private))
                    }
                    _ => false,
                }
        }
        _ => false,
    };
    implementation.type_params.len() == role.owner_type_parameter_count()
        && type_args.len() == role.owner_type_parameter_count()
        && (declaration.visibility == Visibility::Public) == role.owner_is_public()
        && shape
        && expected_owners.contains(
            &super::native_role::canonical_path(hir, symbol)
                .join(".")
                .as_str(),
        )
        && method.defining_module == declaration.defining_module
        && method.name.rsplit('.').next() == role.source_path().rsplit('.').next()
}
