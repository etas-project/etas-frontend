use etas_hir::{HirExpr, HirExprId, HirOwner};
use etas_types::{ItemSignature, Type, TypeId};

/// An internal execution boundary does not introduce a language callable.
#[derive(Clone, Copy, Debug, Default)]
pub struct LexicalExitTypes {
    pub returning: Option<TypeId>,
    pub resuming: Option<TypeId>,
    pub finishing: Option<TypeId>,
}

impl crate::CheckedProgramData {
    pub fn lexical_exit_types(&self, expr: HirExprId) -> Result<LexicalExitTypes, String> {
        let index = self.hir.index();
        let mut owner = HirOwner::Expr(expr);
        let mut result = LexicalExitTypes::default();
        loop {
            owner = match owner {
                HirOwner::Expr(expr) => {
                    if matches!(self.hir.exprs.get(expr), Some(HirExpr::Lambda { .. })) {
                        let Some(Type::Function(flow)) = self
                            .types
                            .expr_types
                            .get(&expr)
                            .and_then(|ty| self.type_store.get(*ty))
                        else {
                            return Err("lexical lambda has no checked return type".into());
                        };
                        result.returning = Some(flow.output);
                        break;
                    }
                    *index
                        .expr_owner
                        .get(&expr)
                        .expect("checked expression owner")
                }
                HirOwner::HandlerArm(arm) => {
                    result.resuming = Some(
                        self.types
                            .handler_actions
                            .get(&arm)
                            .ok_or("lexical Handler arm has no checked action output")?
                            .output,
                    );
                    let HirOwner::Expr(handler) = index.handler_arm_owner[&arm] else {
                        unreachable!("checked Handler arm belongs to its Handler expression")
                    };
                    result.finishing = self
                        .effects
                        .handler_values
                        .get(&handler)
                        .ok_or("lexical Handler has no checked answer contract")?
                        .result;
                    // Return is rejected inside an arm. Resume and finish do
                    // not cross the arm's or a lambda's lexical boundary.
                    break;
                }
                HirOwner::Body(body) => index.body_owner[&body],
                HirOwner::Block(block) => index.block_owner[&block],
                HirOwner::Stmt(stmt) => HirOwner::Block(index.stmt_block[&stmt]),
                HirOwner::Pattern(pattern) => index.pat_owner[&pattern],
                HirOwner::Item(item) => {
                    result.returning = match self.types.item_signatures.get(&item) {
                        Some(ItemSignature::Flow(flow) | ItemSignature::Tool(flow)) => {
                            Some(flow.output)
                        }
                        Some(ItemSignature::Agent(agent)) => {
                            agent.callable().map(|flow| flow.output)
                        }
                        _ => None,
                    };
                    break;
                }
                HirOwner::Module(_) => break,
            };
        }
        Ok(result)
    }
}
