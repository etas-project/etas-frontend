//! Project checked source action signatures into the execution query. The
//! native entry retains dispatch; source owns signatures and default Effect rows.
use etas_hir::{HirItem, HirProgram};
use etas_std::{StdDecl, StdRegistry, StdRegistryBuilder, StdType as S};
use etas_types::{Type as T, TypeId, TypeOutput};
use std::collections::{BTreeMap, HashMap};

pub(super) fn complete(
    hir: &HirProgram,
    types: &TypeOutput,
    registry: &StdRegistry,
) -> Result<StdRegistry, String> {
    let mut actions = BTreeMap::<_, Vec<_>>::new();
    for (_, item) in hir.items.iter() {
        let HirItem::Effect(effect) = item else {
            continue;
        };
        for action in effect.body.actions() {
            let Some(target) = hir.standard_action_targets.get(&action.symbol) else {
                continue;
            };
            let signature = types
                .facts
                .action_signatures
                .get(&action.symbol)
                .ok_or("source action has no checked signature")?;
            let StdDecl::EffectAction(native) = &registry
                .symbol(*target)
                .ok_or("source action execution entry is missing")?
                .decl
            else {
                return Err("source action target is not an execution entry".into());
            };
            let projection = Projection {
                hir,
                types,
                parameters: signature
                    .generic_params
                    .iter()
                    .map(|param| (param.subject, param.name.as_str()))
                    .collect(),
            };
            let mut declaration = native.clone();
            declaration.type_params = signature
                .generic_params
                .iter()
                .map(|param| {
                    Ok(etas_std::StdGenericParam {
                        name: param.name.clone(),
                        kind: match param.kind {
                            etas_types::CallableGenericParamKind::Type => {
                                etas_std::StdGenericParamKind::Type
                            }
                            etas_types::CallableGenericParamKind::Effect => {
                                etas_std::StdGenericParamKind::EffectRow
                            }
                        },
                        bounds: param
                            .bounds
                            .iter()
                            .map(|bound| projection.bound(bound))
                            .collect::<Result<_, String>>()?,
                    })
                })
                .collect::<Result<_, String>>()?;
            declaration.params = signature
                .params
                .iter()
                .map(|ty| projection.ty(*ty))
                .collect::<Result<_, String>>()?;
            declaration.output = projection.ty(signature.output)?;
            declaration.selector_param_names = signature.selector_param_names.clone();
            let defaults = signature
                .default_handler_effects
                .as_ref()
                .map(|row| projection.row(row))
                .transpose()?;
            declaration.default_handler_effect_tail =
                defaults.as_ref().and_then(|row| row.tail.clone());
            declaration.default_handler_effects = defaults.map(|row| row.effects);
            actions.entry(*target).or_default().push(declaration);
        }
    }
    let mut builder = StdRegistryBuilder::from_registry(registry.clone());
    // Only the bootstrap seed contains unselected execution contracts. A
    // completed SDK query owns checked declarations beyond the source units
    // retained in a particular native instantiation.
    if !registry
        .symbols()
        .any(|symbol| registry.declaration_key(symbol.id).is_some())
    {
        builder.retain_source_actions(&actions.keys().copied().collect());
    }
    for (target, declarations) in actions {
        let mut declarations = declarations.into_iter();
        let first = declarations
            .next()
            .expect("checked source action declaration");
        builder.source_action_signature(target, first);
        builder.source_action_overloads(target, declarations.collect());
    }
    builder.try_finish().map_err(|error| error.to_string())
}

struct Projection<'a> {
    hir: &'a HirProgram,
    types: &'a TypeOutput,
    parameters: HashMap<TypeId, &'a str>,
}

impl Projection<'_> {
    fn bound(&self, bound: &etas_types::CheckedSpecBound) -> Result<etas_std::StdSpecRef, String> {
        let path = match &bound.spec {
            etas_types::CheckedSpecRef::Std(path) => path.clone(),
            etas_types::CheckedSpecRef::External(identity) => identity.path.clone(),
            etas_types::CheckedSpecRef::Canonical(identity) => identity
                .module()
                .iter()
                .chain(identity.declaration())
                .cloned()
                .collect(),
            etas_types::CheckedSpecRef::Source(symbol) => {
                let symbol = self
                    .hir
                    .canonical_source_symbol(*symbol)
                    .ok_or("source bound has no declaration")?;
                let data = self
                    .hir
                    .symbols
                    .get(symbol)
                    .ok_or("source bound symbol is absent")?;
                let mut path = self.hir.modules_arena[data.defining_module]
                    .name
                    .as_ref()
                    .ok_or("source bound module has no name")?
                    .segments
                    .iter()
                    .map(|part| part.name.clone())
                    .collect::<Vec<_>>();
                path.push(data.name.clone());
                path
            }
        };
        Ok(etas_std::StdSpecRef {
            path,
            args: bound
                .args
                .iter()
                .map(|ty| self.ty(*ty))
                .collect::<Result<_, String>>()?,
        })
    }

    fn row(&self, row: &etas_types::EffectRowRef) -> Result<etas_std::StdEffectRowExpr, String> {
        use etas_std::StdStaticArg as B;
        use etas_types::EffectArgRef as A;
        Ok(etas_std::StdEffectRowExpr {
            tail: row.tail.clone(),
            effects: row
                .effects
                .iter()
                .map(|effect| {
                    Ok(etas_std::StdEffectRef {
                        path: effect.name.split('.').map(str::to_owned).collect(),
                        args: effect
                            .args
                            .iter()
                            .map(|arg| {
                                Ok(match arg {
                                    A::Type(ty) => B::Type(self.ty(*ty)?),
                                    A::Path(path) => B::Path(path.clone()),
                                    A::String(value) => B::String(value.clone()),
                                    A::Int(value) => B::Int(value.clone()),
                                    A::Wildcard => B::Wildcard,
                                })
                            })
                            .collect::<Result<_, String>>()?,
                    })
                })
                .collect::<Result<_, String>>()?,
        })
    }

    fn ty(&self, ty: TypeId) -> Result<S, String> {
        if let Some(parameter) = self.parameters.get(&ty) {
            return Ok(S::Var((*parameter).into()));
        }
        Ok(
            match self
                .types
                .store
                .get(ty)
                .ok_or("source query type is missing")?
            {
                T::Primitive(primitive) => S::parse(primitive.source_name()),
                T::Nominal(owner) => S::Named(owner.name.clone()),
                T::Enum(owner) => S::Named(owner.name.clone()),
                T::Applied { constructor, args } => {
                    let name = match self.types.store.get(TypeId(constructor.0)) {
                        Some(T::Nominal(owner)) => &owner.name,
                        Some(T::Enum(owner)) => &owner.name,
                        _ => {
                            return Err("source application has no nominal/enum constructor".into());
                        }
                    };
                    S::NamedApplied {
                        name: name.clone(),
                        args: args
                            .iter()
                            .map(|ty| self.ty(*ty))
                            .collect::<Result<_, String>>()?,
                    }
                }
                T::Array(inner) => S::Array(Box::new(self.ty(*inner)?)),
                T::List(inner) => S::List(Box::new(self.ty(*inner)?)),
                T::Slice(inner) => S::Slice(Box::new(self.ty(*inner)?)),
                T::Set(inner) => S::Set(Box::new(self.ty(*inner)?)),
                T::Range { index } => S::Range(Box::new(self.ty(*index)?)),
                T::Option(inner) => S::Option(Box::new(self.ty(*inner)?)),
                T::Map { key, value } => S::Map {
                    key: Box::new(self.ty(*key)?),
                    value: Box::new(self.ty(*value)?),
                },
                T::Result { ok, err } => S::Result {
                    ok: Box::new(self.ty(*ok)?),
                    err: Box::new(self.ty(*err)?),
                },
                T::Tuple(fields) => S::Tuple(
                    fields
                        .iter()
                        .map(|ty| self.ty(*ty))
                        .collect::<Result<_, String>>()?,
                ),
                T::Record(record) => S::Record(
                    record
                        .fields
                        .iter()
                        .map(|field| {
                            Ok(etas_std::StdRecordField {
                                name: field.name.clone(),
                                ty: self.ty(field.ty)?,
                            })
                        })
                        .collect::<Result<_, String>>()?,
                ),
                T::Function(flow) => S::Function {
                    input: flow
                        .input
                        .iter()
                        .map(|ty| self.ty(*ty))
                        .collect::<Result<_, String>>()?,
                    output: Box::new(self.ty(flow.output)?),
                    effects: flow
                        .effects
                        .as_ref()
                        .map(|row| self.row(row))
                        .transpose()?
                        .unwrap_or_default(),
                },
                T::Handler(handler) => S::Handler {
                    handled: self.row(&handler.handled)?,
                    produced: match &handler.produced {
                        etas_types::HandlerProducedEffects::Infer => None,
                        etas_types::HandlerProducedEffects::Explicit(row) => Some(self.row(row)?),
                    },
                    result: handler
                        .result
                        .map(|ty| self.ty(ty).map(Box::new))
                        .transpose()?,
                },
                T::Existential { bounds } => S::Existential {
                    bounds: bounds
                        .iter()
                        .map(|bound| self.bound(bound))
                        .collect::<Result<_, String>>()?,
                },
                T::Task(task) => S::Task {
                    output: Box::new(self.ty(task.output)?),
                    effects: self.row(&task.effects)?,
                },
                other => {
                    return Err(format!(
                        "checked source action type {other:?} is not representable in the std query"
                    ));
                }
            },
        )
    }
}
