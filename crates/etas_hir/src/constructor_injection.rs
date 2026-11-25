use std::collections::HashSet;

use crate::*;

impl HirProgram {
    /// Resolve defaults at their declaration, then retain the ordinary perform
    /// expressions in construction order for type/effect analysis and execution.
    pub(crate) fn elaborate_constructor_injections(&mut self) {
        let records = self
            .exprs
            .iter()
            .filter_map(|(expr, value)| match value {
                HirExpr::Record(record) if record.injections.is_none() => {
                    Some((expr, record.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for (expr, mut record) in records {
            let Some(ResolveResult::Resolved(symbol)) =
                record.path.as_ref().map(|path| &path.resolution)
            else {
                continue;
            };
            let Some((owner, fields)) = self.injection_fields(*symbol) else {
                continue;
            };
            let supplied = record
                .fields
                .iter()
                .map(|field| match field {
                    HirFieldInit::Named { name, .. } | HirFieldInit::Shorthand { name, .. } => {
                        name.clone()
                    }
                })
                .collect::<HashSet<_>>();
            let mut values = Vec::new();
            for field in fields {
                let name = self
                    .symbols
                    .get(field.symbol)
                    .expect("declared record field")
                    .name
                    .clone();
                if supplied.contains(&name) {
                    continue;
                }
                let Some(template) = field.annotations.iter().find(|annotation| {
                    matches!(annotation.path.segments.as_slice(), [segment] if segment.name == "inject")
                }).and_then(|annotation| match annotation.args.as_slice() {
                    [HirAnnotationArg::Positional { value, .. }] => Some(*value),
                    _ => None,
                }) else { continue; };
                let HirExpr::Perform {
                    mut action,
                    generic_args,
                    args,
                    ..
                } = self.exprs[template].clone()
                else {
                    continue;
                };
                action.effect = self.clone_injection_effect(action.effect);
                let generic_args = generic_args
                    .into_iter()
                    .map(|argument| match argument {
                        HirGenericArg::Type(ty) => {
                            HirGenericArg::Type(self.clone_injection_type(ty))
                        }
                        HirGenericArg::EffectRow(row) => {
                            HirGenericArg::EffectRow(self.clone_injection_row(row))
                        }
                        wildcard => wildcard,
                    })
                    .collect();
                let value = self.exprs.alloc(HirExpr::Perform {
                    action,
                    generic_args,
                    args,
                    span: record.span,
                });
                let origin = match self
                    .source_map
                    .expr_sources
                    .get(&expr)
                    .and_then(HirOrigin::primary_syntax)
                {
                    Some(primary) => HirOrigin::Desugared {
                        primary,
                        related: self
                            .source_map
                            .expr_sources
                            .get(&template)
                            .and_then(HirOrigin::primary_syntax)
                            .into_iter()
                            .collect(),
                        desugaring: DesugaringKind::ConstructorInjection,
                    },
                    None => HirOrigin::Synthetic {
                        span: record.span,
                        reason: SyntheticOriginReason::LoweringGenerated,
                    },
                };
                self.source_map.expr_sources.insert(value, origin);
                record.fields.push(HirFieldInit::Named {
                    name,
                    value,
                    span: record.span,
                });
                values.push(value);
            }
            if !values.is_empty() {
                let parameters = match &self.items[owner] {
                    HirItem::Type(declaration) => declaration.type_params.clone(),
                    HirItem::Agent(declaration) => declaration.type_params.clone(),
                    _ => unreachable!("injection owner has stored nominal fields"),
                };
                record.injections = Some(HirRecordInjections { parameters, values });
                self.exprs[expr] = HirExpr::Record(record);
            }
        }
    }

    fn injection_fields(&self, mut symbol: SymbolId) -> Option<(HirItemId, Vec<HirFieldDecl>)> {
        let mut visited = HashSet::new();
        loop {
            symbol = self.canonical_source_symbol(symbol)?;
            if !visited.insert(symbol) {
                return None;
            }
            let SymbolDef::Item { item } = self.symbols.get(symbol)?.def else {
                return None;
            };
            let representation = match &self.items[item] {
                HirItem::Type(declaration) => match declaration.body {
                    HirTypeDeclBody::Representation(ty) => ty,
                    _ => return None,
                },
                HirItem::Agent(declaration) => declaration.state?,
                HirItem::TypeAlias(alias) => {
                    let HirType::Path { path, .. } = &self.types[alias.target] else {
                        return None;
                    };
                    let ResolveResult::Resolved(target) = path.resolution else {
                        return None;
                    };
                    symbol = target;
                    continue;
                }
                _ => return None,
            };
            return match &self.types[representation] {
                HirType::Record { fields, .. } => Some((item, fields.clone())),
                _ => None,
            };
        }
    }

    // Every construction owns its selector type references. Checked facts for
    // Config<i32> must not overwrite those for Config<string> or the declaration.
    fn clone_injection_type(&mut self, source: HirTypeId) -> HirTypeId {
        let mut ty = self.types[source].clone();
        match &mut ty {
            HirType::Path { args, .. } | HirType::Tuple { elems: args, .. } => {
                for argument in args {
                    *argument = self.clone_injection_type(*argument);
                }
            }
            HirType::Existential { bounds, .. } => {
                for bound in bounds {
                    for argument in &mut bound.args {
                        *argument = self.clone_injection_type(*argument);
                    }
                }
            }
            HirType::Handler {
                handled,
                produced,
                result,
                ..
            } => {
                *handled = self.clone_injection_row(handled.clone());
                if let HirHandlerProducedEffects::Explicit(row) = produced {
                    *row = self.clone_injection_row(row.clone());
                }
                if let Some(result) = result {
                    *result = self.clone_injection_type(*result);
                }
            }
            HirType::Arrow {
                effect,
                input,
                output,
                ..
            } => {
                if let Some(row) = effect {
                    *row = self.clone_injection_row(row.clone());
                }
                *input = self.clone_injection_type(*input);
                *output = self.clone_injection_type(*output);
            }
            HirType::Record { fields, .. } => {
                for field in fields {
                    field.ty = self.clone_injection_type(field.ty);
                }
            }
            HirType::Array { element, .. } => {
                *element = self.clone_injection_type(*element);
            }
            HirType::Refined { base, .. } => {
                *base = self.clone_injection_type(*base);
            }
            HirType::Metadata { .. } | HirType::Primitive { .. } | HirType::Error { .. } => {}
        }
        let target = self.types.alloc(ty);
        if let Some(origin) = self.source_map.type_sources.get(&source).cloned() {
            self.source_map.type_sources.insert(target, origin);
        }
        target
    }

    fn clone_injection_row(&mut self, mut row: HirEffectRow) -> HirEffectRow {
        row.effects = row
            .effects
            .into_iter()
            .map(|effect| self.clone_injection_effect(effect))
            .collect();
        row
    }

    fn clone_injection_effect(&mut self, mut effect: HirEffectRef) -> HirEffectRef {
        for argument in &mut effect.args {
            match argument {
                HirEffectArg::Type(ty) => *ty = self.clone_injection_type(*ty),
                HirEffectArg::PatternBinding { bounds, .. } => {
                    for bound in bounds {
                        for ty in &mut bound.args {
                            *ty = self.clone_injection_type(*ty);
                        }
                    }
                }
                _ => {}
            }
        }
        effect
    }
}
