use crate::*;
use std::collections::HashSet;

impl HirProgram {
    /// A type stage is an ordinary closure. Constructing inside its body keeps
    /// instance creation and request submission on the normal checked paths,
    /// and each invocation gets a fresh instance.
    pub fn elaborate_type_stages(
        &mut self,
        imported: &HashSet<HirExprId>,
    ) -> Result<(), HirTreeIndexError> {
        let mut stages = self
            .exprs
            .iter()
            .flat_map(|(_, expression)| match expression {
                HirExpr::Pipeline { stages, .. } | HirExpr::StageCompose { stages, .. } => {
                    stages.iter().map(|stage| stage.expr).collect::<Vec<_>>()
                }
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        stages.sort_unstable();
        stages.dedup();
        let targets = stages
            .into_iter()
            .filter_map(|stage| {
                let HirExpr::Path(path) = &self.exprs[stage] else {
                    return None;
                };
                let local = match path.resolution {
                    ResolveResult::Resolved(symbol) => self.local_type_stage(symbol),
                    _ => false,
                };
                (imported.contains(&stage) || local).then(|| (stage, path.clone()))
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(());
        }
        let owners = {
            let view = HirTreeView::try_new(self)?;
            targets
                .into_iter()
                .map(|(stage, path)| {
                    let scope = view
                        .enclosing_scope(HirNodeRef::Expr(stage))
                        .expect("stage lexical scope");
                    let item = view.index().expr_item[&stage];
                    let module = view.index().item_module[&item];
                    (stage, path, scope, item, module)
                })
                .collect::<Vec<_>>()
        };
        for (stage, path, parent, item, module) in owners {
            let span = self.exprs[stage].span(&self.blocks);
            let origin = self
                .source_map
                .expr_sources
                .get(&stage)
                .and_then(HirOrigin::primary_syntax)
                .map(|primary| HirOrigin::Desugared {
                    primary,
                    related: Vec::new(),
                    desugaring: DesugaringKind::TypeStage,
                })
                .unwrap_or(HirOrigin::Synthetic {
                    span,
                    reason: SyntheticOriginReason::LoweringGenerated,
                });
            let scope = self
                .scopes
                .alloc(Some(parent), ScopeOwner::Lambda(stage), span);
            let parameter = self.symbols.alloc(SymbolData {
                name: "$stage_input".into(),
                kind: SymbolKind::Param,
                visibility: Visibility::Private,
                defining_module: module,
                defining_item: Some(item),
                declared_type: None,
                definition_span: span,
                def: SymbolDef::Param {
                    owner: item,
                    param_index: 0,
                    pattern: None,
                    ty: None,
                },
            });
            self.scopes.insert(scope, "$stage_input".into(), parameter);
            self.source_map
                .symbol_sources
                .insert(parameter, origin.clone());
            let mut argument_path = unresolved_path_from_segments(&["$stage_input"], span);
            argument_path.resolution = ResolveResult::Resolved(parameter);
            let argument = self.exprs.alloc(HirExpr::Path(argument_path));
            let instance = self.exprs.alloc(HirExpr::Record(HirRecordExpr {
                base: None,
                path: Some(path),
                generic_args: Vec::new(),
                fields: Vec::new(),
                injections: None,
                span,
            }));
            let invocation = self.exprs.alloc(HirExpr::MethodCall {
                receiver: instance,
                method: "run".into(),
                generic_args: Vec::new(),
                args: vec![HirArg::Positional(argument)],
                span,
            });
            for expression in [stage, argument, instance, invocation] {
                self.source_map
                    .expr_sources
                    .insert(expression, origin.clone());
            }
            self.exprs[stage] = HirExpr::Lambda {
                params: vec![parameter],
                body: HirLambdaBody::Expr(invocation),
                scope,
                span,
            };
        }
        Ok(())
    }

    fn local_type_stage(&self, symbol: SymbolId) -> bool {
        let mut symbol = symbol;
        let mut seen = HashSet::new();
        let representation = loop {
            let Some(canonical) = self.canonical_source_symbol(symbol) else {
                return false;
            };
            symbol = canonical;
            if !seen.insert(symbol) {
                return false;
            }
            let Some(SymbolDef::Item { item }) = self.symbols.get(symbol).map(|symbol| &symbol.def)
            else {
                return false;
            };
            match &self.items[*item] {
                HirItem::Agent(agent) => break agent.state,
                HirItem::Type(declaration) => {
                    break match declaration.body {
                        HirTypeDeclBody::Representation(ty) => Some(ty),
                        _ => None,
                    };
                }
                HirItem::TypeAlias(alias) => {
                    let HirType::Path { path, .. } = &self.types[alias.target] else {
                        return false;
                    };
                    let ResolveResult::Resolved(target) = path.resolution else {
                        return false;
                    };
                    symbol = target;
                }
                _ => return false,
            }
        };
        if !matches!(representation.map(|ty| &self.types[ty]), Some(HirType::Record { fields, .. }) if fields.is_empty())
        {
            return false;
        }
        self.items.iter().any(|(_, item)| {
            let HirItem::Impl(implementation) = item else {
                return false;
            };
            let HirImplTarget::Inherent { target, .. } = &implementation.target else {
                return false;
            };
            let ResolveResult::Resolved(owner) = target.resolution else {
                return false;
            };
            if self.canonical_source_symbol(owner) != Some(symbol) {
                return false;
            }
            implementation.items.iter().any(|member| {
                let flow = match member {
                    HirImplItem::Flow(flow) => flow,
                    HirImplItem::Callable { item } => match &self.items[*item] {
                        HirItem::Flow(flow) => flow,
                        _ => return false,
                    },
                    _ => return false,
                };
                self.symbols.get(flow.symbol).is_some_and(|symbol| {
                    symbol.name.rsplit('.').next() == Some("run")
                        && symbol.visibility == Visibility::Public
                }) && flow.receiver_mode != HirReceiverMode::None
            })
        })
    }
}
