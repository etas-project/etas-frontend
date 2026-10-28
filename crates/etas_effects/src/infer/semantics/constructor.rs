use etas_hir::{HirItem, SymbolDef, SymbolId};
use etas_std::{StdDecl, StdSymbolKind};
use etas_types::{SymbolTypeFact, Type, TypeId};

use super::engine::EffectSemantics;

impl EffectSemantics<'_> {
    pub(super) fn checked_empty_constructor_value(
        &self,
        expr: etas_hir::HirExprId,
        symbol: SymbolId,
    ) -> bool {
        if self.is_typed_flow_expr(expr) {
            return false;
        }
        if self.checked_enum_constructor(symbol) {
            return matches!(self.type_symbols.symbol_fact(self.hir, &self.types.facts, symbol), Some(SymbolTypeFact::Flow { signature }) if signature.params.is_empty());
        }
        let Some(SymbolDef::ImportAlias { path, .. }) =
            self.hir.symbols.get(symbol).map(|symbol| &symbol.def)
        else {
            return false;
        };
        self.std_registry
            .lookup_qualified(path)
            .is_some_and(|declaration| {
                declaration.intrinsic.as_ref().is_some_and(|descriptor| {
                    descriptor.latent_effect == etas_std::IntrinsicLatentEffect::TransparentFirstArg
                }) && declaration.kind == StdSymbolKind::Constructor
                    && matches!(self.type_symbols.symbol_fact(self.hir, &self.types.facts, symbol), Some(SymbolTypeFact::Flow { signature }) if signature.params.is_empty())
            })
    }

    pub(super) fn checked_enum_constructor(&self, symbol: SymbolId) -> bool {
        let Some(definition) = self.hir.symbols.get(symbol) else {
            return false;
        };
        let declared = match &definition.def {
            SymbolDef::EnumVariant { enum_item, variant_index } => {
                matches!(self.hir.items.get(*enum_item), Some(HirItem::Enum(decl)) if decl.variants.get(*variant_index as usize).is_some_and(|variant| variant.symbol == symbol))
            }
            SymbolDef::ImportAlias { path, .. } => self.std_registry.lookup_qualified(path).is_some_and(|symbol| {
                symbol.enum_owner.is_some() && symbol.intrinsic.is_none()
                    && matches!(&symbol.decl, StdDecl::Flow(flow) if flow.public_effects.is_empty() && flow.requested_actions.is_empty())
            }),
            _ => false,
        };
        if !declared {
            return false;
        }
        let Some(SymbolTypeFact::Flow { signature }) =
            self.type_symbols
                .symbol_fact(self.hir, &self.types.facts, symbol)
        else {
            return false;
        };
        if signature
            .effects
            .iter()
            .chain(&signature.requested_actions)
            .any(|row| !row.effects.is_empty() || row.tail.is_some())
        {
            return false;
        }
        let constructor = match self.types.store.get(signature.output) {
            Some(Type::Enum(_)) => signature.output,
            Some(Type::Applied { constructor, .. })
                if matches!(
                    self.types.store.get(TypeId(constructor.0)),
                    Some(Type::Enum(_))
                ) =>
            {
                TypeId(constructor.0)
            }
            _ => return false,
        };
        self.types.facts.enum_layouts.contains_key(&constructor)
    }
}
