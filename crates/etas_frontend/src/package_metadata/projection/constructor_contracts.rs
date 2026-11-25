use super::*;
use etas_hir::{HirAnnotationArg, HirEffectArg, HirExpr, HirGenericArg, HirType, HirTypeDeclBody};
use etas_package_metadata::{
    ConstructorContract, ConstructorField, ConstructorGenericArgument, ConstructorInjection,
    GenericParam, GenericParamKind,
};

impl ProjectMetadataProjection<'_> {
    pub(super) fn constructor_contract(
        &self,
        path: Vec<String>,
        item: HirItemId,
    ) -> Result<Option<ConstructorContract>, PackageMetadataError> {
        let (representation, parameters, agent) = match &self.checked.hir.items[item] {
            HirItem::Type(decl) => match decl.body {
                HirTypeDeclBody::Representation(ty) => (ty, &decl.type_params, false),
                _ => return Ok(None),
            },
            HirItem::Agent(decl) => match decl.state {
                Some(ty) => (ty, &decl.type_params, true),
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        let HirType::Record { fields, .. } = &self.checked.hir.types[representation] else {
            return Ok(None);
        };
        let template = |field: &etas_hir::HirFieldDecl| {
            field.annotations.iter().find(|annotation| matches!(annotation.path.segments.as_slice(), [segment] if segment.name == "inject"))
                .and_then(|annotation| match annotation.args.as_slice() {
                    [HirAnnotationArg::Positional { value, .. }] => Some(*value),
                    _ => None,
                })
        };
        if !agent && !fields.iter().any(|field| template(field).is_some()) {
            return Ok(None);
        }
        let names = parameters
            .iter()
            .map(|parameter| {
                self.checked
                    .symbols
                    .get(*parameter)
                    .expect("declared constructor type parameter")
                    .name
                    .clone()
            })
            .collect::<BTreeSet<_>>();
        let generic_params = parameters
            .iter()
            .map(|parameter| {
                let symbol = &self
                    .checked
                    .symbols
                    .get(*parameter)
                    .expect("declared constructor type parameter");
                Ok(GenericParam {
                    name: symbol.name.clone(),
                    kind: if symbol.kind == etas_hir::SymbolKind::EffectParam {
                        GenericParamKind::Effect
                    } else {
                        GenericParamKind::Type
                    },
                    bounds: self
                        .checked
                        .types
                        .type_param_bounds
                        .get(parameter)
                        .into_iter()
                        .flatten()
                        .map(|bound| {
                            self.checked_spec_bound_metadata(
                                &etas_types::CheckedSpecBound {
                                    spec: etas_types::CheckedSpecRef::Source(bound.spec_symbol),
                                    args: bound.args.clone(),
                                },
                                &names,
                            )
                        })
                        .collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, PackageMetadataError>>()?;
        let fields = fields
            .iter()
            .map(|field| {
                let ty = self.constructor_type(field.ty, &names)?;
                let injection = template(field)
                    .map(|template| {
                        let HirExpr::Perform {
                            action,
                            generic_args,
                            args,
                            ..
                        } = &self.checked.hir.exprs[template]
                        else {
                            return Err(invalid(
                                "constructor provider has no checked perform expression",
                            ));
                        };
                        if !args.is_empty()
                            || self
                                .checked
                                .types
                                .expression_producer_type(template)
                                .is_none()
                        {
                            return Err(invalid(
                                "constructor provider has no checked zero-argument action contract",
                            ));
                        }
                        let action_path = self
                            .checked
                            .types
                            .action_calls
                            .get(&template)
                            .and_then(|call| self.action_path(call.action_symbol))
                            .or_else(|| match action.action_symbol {
                                etas_hir::ResolveResult::Resolved(symbol) => {
                                    self.action_path(symbol)
                                }
                                _ => None,
                            })
                            .map(Ok)
                            .unwrap_or_else(|| {
                                let mut path = self.effect_ref_path(&action.effect)?;
                                path.push(action.action.clone());
                                Ok::<_, PackageMetadataError>(path)
                            })?;
                        Ok(ConstructorInjection {
                            action: MetadataEffectRef {
                                path: action_path,
                                args: action
                                    .effect
                                    .args
                                    .iter()
                                    .map(|arg| self.constructor_effect_arg(arg, &names))
                                    .collect::<Result<_, _>>()?,
                            },
                            generic_args: generic_args
                                .iter()
                                .map(|arg| {
                                    Ok(match arg {
                                        HirGenericArg::Type(ty) => {
                                            ConstructorGenericArgument::Type(
                                                self.constructor_type(*ty, &names)?,
                                            )
                                        }
                                        HirGenericArg::EffectRow(row) => {
                                            ConstructorGenericArgument::EffectRow(
                                                self.constructor_row(row, &names)?,
                                            )
                                        }
                                        HirGenericArg::Wildcard { .. } => {
                                            ConstructorGenericArgument::Infer
                                        }
                                    })
                                })
                                .collect::<Result<_, PackageMetadataError>>()?,
                        })
                    })
                    .transpose()?;
                Ok(ConstructorField {
                    name: self
                        .checked
                        .symbols
                        .get(field.symbol)
                        .expect("declared constructor field")
                        .name
                        .clone(),
                    ty,
                    injection,
                })
            })
            .collect::<Result<_, PackageMetadataError>>()?;
        Ok(Some(ConstructorContract {
            path,
            generic_params,
            fields,
            agent,
        }))
    }

    fn constructor_type(
        &self,
        ty: etas_hir::HirTypeId,
        names: &BTreeSet<String>,
    ) -> Result<MetadataType, PackageMetadataError> {
        let ty = self
            .checked
            .types
            .type_refs
            .get(&ty)
            .copied()
            .ok_or_else(|| invalid("constructor declaration type has no checked type fact"))?;
        self.metadata_type_in_generic_scope(ty, names)
    }

    fn constructor_row(
        &self,
        row: &etas_hir::HirEffectRow,
        names: &BTreeSet<String>,
    ) -> Result<MetadataEffectRow, PackageMetadataError> {
        Ok(MetadataEffectRow {
            effects: row
                .effects
                .iter()
                .map(|effect| {
                    Ok(MetadataEffectRef {
                        path: self.effect_ref_path(effect)?,
                        args: effect
                            .args
                            .iter()
                            .map(|arg| self.constructor_effect_arg(arg, names))
                            .collect::<Result<_, _>>()?,
                    })
                })
                .collect::<Result<_, PackageMetadataError>>()?,
            tail: row.tail.as_ref().map(|path| path_segments(path).join(".")),
        })
    }

    fn constructor_static_path(
        &self,
        path: &etas_hir::ResolvedPath,
    ) -> Result<Vec<String>, PackageMetadataError> {
        etas_hir::static_selector_path_segments(&self.checked.hir, path, |symbol| {
            self.checked.hir.canonical_source_symbol(symbol)
        })
        .ok_or_else(|| invalid("constructor static selector has no checked declaration path"))
    }

    fn constructor_effect_arg(
        &self,
        arg: &HirEffectArg,
        names: &BTreeSet<String>,
    ) -> Result<MetadataEffectArg, PackageMetadataError> {
        Ok(match arg {
            HirEffectArg::Type(ty) => {
                if self.checked.types.type_refs.contains_key(ty) {
                    MetadataEffectArg {
                        kind: MetadataEffectArgKind::Type,
                        ty: Some(self.constructor_type(*ty, names)?),
                        ..Default::default()
                    }
                } else {
                    let HirType::Path { path, .. } = &self.checked.hir.types[*ty] else {
                        return Err(invalid(
                            "constructor selector has no checked type or static path",
                        ));
                    };
                    MetadataEffectArg {
                        kind: MetadataEffectArgKind::Path,
                        path: self.constructor_static_path(path)?,
                        ..Default::default()
                    }
                }
            }
            HirEffectArg::Path(path) => MetadataEffectArg {
                kind: MetadataEffectArgKind::Path,
                path: self.constructor_static_path(path)?,
                ..Default::default()
            },
            HirEffectArg::String { value, .. } => MetadataEffectArg {
                kind: MetadataEffectArgKind::String,
                value: value.clone(),
                ..Default::default()
            },
            HirEffectArg::Int { text, .. } => MetadataEffectArg {
                kind: MetadataEffectArgKind::Int,
                value: text.clone(),
                ..Default::default()
            },
            HirEffectArg::Wildcard { .. } => MetadataEffectArg {
                kind: MetadataEffectArgKind::Wildcard,
                ..Default::default()
            },
            HirEffectArg::PatternBinding { .. } => {
                return Err(invalid(
                    "constructor provider cannot declare a selector pattern",
                ));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn published_constructor_contracts_preserve_source_order_and_selector_parameters() {
        let source = r#"
module library.wiring;
public effect Supply { action first() -> i32; action value<T>() -> T; }
public type Settings = { @inject(Supply.first) first: i32, explicit: i32, @inject(Supply.first) second: i32 };
public type Box<T> = { marker: T, @inject(Supply.value<T>) value: T };
public agent Counter = { @inject(Supply.first) n: i32 };
impl Counter { public flow get(self) -> i32 ![] { return self.n; } }
flow main() -> unit { return; }
"#;
        let output = crate::Frontend.check_project(crate::ProjectInput::single_source(
            crate::SourceInput::anonymous(source),
        ));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let artifact = crate::build_package_metadata_artifact_from_checked(
            crate::PackageMetadataBuildInput {
                definition_origin: Some(
                    etas_code::PackageKey::from_name_and_version("constructor-wiring", "0.1.0")
                        .unwrap(),
                ),
                package_id: "constructor-wiring".into(),
                package_version: "0.1.0".into(),
                package_edition: "2026".into(),
                compiler_version: crate::FRONTEND_COMPILER_VERSION.into(),
                source_payload_hash: "source".into(),
                manifest_hash: "manifest".into(),
                dependency_lock_hash: "lock".into(),
                bins: vec![],
                dependencies: vec![],
                tool_bindings: vec![],
            },
            &output.checked.unwrap(),
        )
        .unwrap();
        let (_, decoded) = etas_package_metadata::package_metadata_from_artifact(
            std::path::Path::new("constructor.etasmeta"),
            &artifact.bytes,
        )
        .unwrap();
        let contracts = decoded.public_metadata.constructor_contracts;
        let settings = contracts
            .iter()
            .find(|contract| contract.path.last().is_some_and(|name| name == "Settings"))
            .unwrap();
        assert_eq!(
            settings
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "explicit", "second"]
        );
        assert!(settings.fields[1].injection.is_none());
        assert_eq!(
            settings.fields[0].injection.as_ref().unwrap().action.path,
            ["library", "wiring", "Supply", "first"]
        );
        let generic = contracts
            .iter()
            .find(|contract| contract.path.last().is_some_and(|name| name == "Box"))
            .unwrap();
        assert_eq!(generic.generic_params[0].name, "T");
        // The selector after the action name is retained in the ordinary
        // perform argument position, distinct from effect-owner arguments.
        let [etas_package_metadata::ConstructorGenericArgument::Type(selector)] = generic.fields[1]
            .injection
            .as_ref()
            .unwrap()
            .generic_args
            .as_slice()
        else {
            panic!("generic provider lost its checked selector template");
        };
        assert_eq!(selector.kind, etas_package_metadata::TypeKind::Var);
        assert_eq!(selector.name, "T");
        let agent = contracts.iter().find(|contract| contract.agent).unwrap();
        assert_eq!(agent.path, ["library", "wiring", "Counter"]);
        assert_eq!(agent.fields[0].name, "n");
    }
}
