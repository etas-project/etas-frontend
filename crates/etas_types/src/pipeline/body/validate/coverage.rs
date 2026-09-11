use super::BodyPipelineState;
use crate::{PrimitiveType, Type, TypeId, pipeline::context::TypePipelineContext};
use etas_core::{Diagnostic, Severity, Span, TypeDiagnosticCode};
use etas_hir::{HirLiteral, HirPat, HirPatId, ResolveResult, ResolvedPath, SymbolDef};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Pattern {
    Any,
    Constructor(Tag, Vec<Pattern>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Tag {
    Structural(&'static str),
    Variant { owner: TypeId, index: usize },
    Literal(String),
    Invalid,
}

#[derive(Clone)]
struct Constructor {
    tag: Tag,
    fields: Vec<TypeId>,
    names: Option<Vec<String>>,
}

pub(super) fn validate(
    ctx: &mut TypePipelineContext<'_>,
    state: &BodyPipelineState,
    ty: TypeId,
    arms: &[HirPatId],
    span: Span,
) {
    let ty = match crate::substitute_type_params(
        &mut ctx.interner,
        ty,
        &state.solver_report.named_substitutions,
        &state.solver_report.substitutions,
    ) {
        Ok(ty) => ty,
        Err(error) => {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                error.to_string(),
            ));
            return;
        }
    };
    let mut coverage = Coverage {
        ctx,
        active: HashSet::new(),
        span,
        failed: false,
    };
    let mut matrix = Vec::new();
    for &arm in arms {
        let pattern = coverage.pattern(arm, ty);
        if coverage.failed {
            return;
        }
        let useful = coverage.useful(&matrix, std::slice::from_ref(&pattern), &[ty]);
        if coverage.failed {
            return;
        }
        if !useful {
            let mut diagnostic = Diagnostic::type_check(
                TypeDiagnosticCode::RedundantMatchArm,
                coverage.ctx.hir.pats[arm].span(),
                "match arm is unreachable",
            );
            diagnostic.severity = Severity::Warning;
            coverage.ctx.diagnostics.push(diagnostic);
        }
        matrix.push(vec![pattern]);
    }
    let missing = coverage.useful(&matrix, &[Pattern::Any], &[ty]);
    if !coverage.failed && missing {
        coverage.ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::NonExhaustiveMatch,
            span,
            "match does not cover all values; add the missing payload cases or a wildcard arm",
        ));
    }
}

type Query = (Vec<Vec<Pattern>>, Vec<Pattern>, Vec<TypeId>);
struct Coverage<'a, 'hir> {
    ctx: &'a mut TypePipelineContext<'hir>,
    active: HashSet<Query>,
    span: Span,
    failed: bool,
}

impl Coverage<'_, '_> {
    fn incomplete(&mut self, message: impl Into<String>) {
        self.failed = true;
        self.ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            self.span,
            message,
        ));
    }

    fn constructors(&mut self, ty: TypeId) -> Option<Vec<Constructor>> {
        let ctor = |tag: &'static str, fields| Constructor {
            tag: Tag::Structural(tag),
            fields,
            names: None,
        };
        let Some(data) = self.ctx.interner.store().get(ty).cloned() else {
            self.incomplete("match scrutinee has no checked type");
            return None;
        };
        let (base, args) = match &data {
            Type::Applied { constructor, args } => (TypeId(constructor.0), args.clone()),
            _ => (ty, Vec::new()),
        };
        if let Some(layout) = self.ctx.signature_facts.enum_layouts.get(&base).cloned() {
            let substitutions = layout
                .type_params
                .into_iter()
                .zip(args)
                .collect::<HashMap<_, _>>();
            let variants = layout
                .variants
                .into_iter()
                .enumerate()
                .map(|(index, variant)| {
                    let fields = variant
                        .fields
                        .into_iter()
                        .map(|field| {
                            crate::substitute_named_params(
                                &mut self.ctx.interner,
                                field,
                                &substitutions,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(Constructor {
                        tag: Tag::Variant { owner: base, index },
                        fields,
                        names: variant.field_names,
                    })
                })
                .collect::<Result<Vec<_>, crate::TypeSubstitutionError>>();
            return match variants {
                Ok(variants) => Some(variants),
                Err(error) => {
                    self.incomplete(error.to_string());
                    None
                }
            };
        }
        match data {
            Type::Primitive(PrimitiveType::Bool) => {
                Some(vec![ctor("true", vec![]), ctor("false", vec![])])
            }
            Type::Primitive(PrimitiveType::Never) => Some(Vec::new()),
            Type::Primitive(PrimitiveType::Unit) => Some(vec![ctor("tuple", vec![])]),
            Type::Option(inner) => Some(vec![ctor("Some", vec![inner]), ctor("None", vec![])]),
            Type::Result { ok, err } => Some(vec![ctor("Ok", vec![ok]), ctor("Err", vec![err])]),
            Type::Tuple(fields) => Some(vec![ctor("tuple", fields)]),
            Type::Record(record) => Some(vec![Constructor {
                tag: Tag::Structural("record"),
                fields: record.fields.iter().map(|f| f.ty).collect(),
                names: Some(record.fields.into_iter().map(|f| f.name).collect()),
            }]),
            Type::Nominal(_) | Type::Applied { .. } => {
                let representation = match crate::applied_representation(&mut self.ctx.interner, ty)
                {
                    Ok(representation) => representation?,
                    Err(error) => {
                        self.incomplete(error.to_string());
                        return None;
                    }
                };
                // Projection unfolds a single record layer; recursive fields remain identities.
                if let Some(Type::Record(record)) =
                    self.ctx.interner.store().get(representation).cloned()
                {
                    Some(vec![Constructor {
                        tag: Tag::Structural("record"),
                        fields: record.fields.iter().map(|f| f.ty).collect(),
                        names: Some(record.fields.into_iter().map(|f| f.name).collect()),
                    }])
                } else {
                    None
                }
            }
            Type::Enum(_) => {
                self.incomplete("enum match is missing its checked variant layout");
                None
            }
            _ => None,
        }
    }

    fn pattern(&mut self, id: HirPatId, ty: TypeId) -> Pattern {
        match self.ctx.hir.pats[id].clone() {
            HirPat::Binding { .. } | HirPat::Wildcard { .. } => Pattern::Any,
            HirPat::Literal(literal) => Pattern::Constructor(
                match literal {
                    HirLiteral::Bool { value, .. } => {
                        Tag::Structural(if value { "true" } else { "false" })
                    }
                    HirLiteral::Int { text, .. } => Tag::Literal(format!("integer:{text}")),
                    HirLiteral::String { value, .. } => Tag::Literal(format!("string:{value}")),
                    HirLiteral::Char { value, .. } => Tag::Literal(format!("char:{value}")),
                    HirLiteral::Float { text, .. } => Tag::Literal(format!("float:{text}")),
                },
                Vec::new(),
            ),
            HirPat::Tuple { elems, .. } => self.positional(Tag::Structural("tuple"), elems, ty),
            HirPat::Variant { path, args, .. } => {
                let tag = self.variant_tag(&path, ty);
                self.positional(tag, args, ty)
            }
            HirPat::Record { path, fields, .. } => {
                let Some(constructors) = self.constructors(ty) else {
                    self.incomplete("record pattern has no checked layout");
                    return Pattern::Constructor(Tag::Invalid, vec![]);
                };
                let tag = if constructors
                    .iter()
                    .any(|c| c.tag == Tag::Structural("record"))
                {
                    Tag::Structural("record")
                } else if let Some(path) = path {
                    self.variant_tag(&path, ty)
                } else {
                    self.incomplete("enum record pattern has no checked constructor");
                    Tag::Invalid
                };
                let Some(constructor) = constructors.into_iter().find(|c| c.tag == tag) else {
                    self.incomplete("record pattern does not match a checked constructor layout");
                    return Pattern::Constructor(Tag::Invalid, vec![]);
                };
                let Some(names) = constructor.names else {
                    self.incomplete("named pattern requires a checked named-field layout");
                    return Pattern::Constructor(Tag::Invalid, vec![]);
                };
                let patterns = names
                    .iter()
                    .zip(constructor.fields)
                    .map(|(name, ty)| {
                        fields
                            .iter()
                            .find(|field| field.name == *name)
                            .and_then(|field| field.pat)
                            .map(|pat| self.pattern(pat, ty))
                            .unwrap_or(Pattern::Any)
                    })
                    .collect();
                Pattern::Constructor(tag, patterns)
            }
            HirPat::Error { .. } => Pattern::Constructor(Tag::Invalid, vec![]),
        }
    }

    fn positional(&mut self, tag: Tag, args: Vec<HirPatId>, ty: TypeId) -> Pattern {
        let Some(fields) = self
            .constructors(ty)
            .and_then(|ctors| ctors.into_iter().find(|c| c.tag == tag))
            .map(|c| c.fields)
        else {
            self.incomplete("pattern constructor has no checked payload layout");
            return Pattern::Constructor(Tag::Invalid, vec![]);
        };
        if fields.len() != args.len() {
            self.incomplete("pattern payload arity does not match its checked constructor");
            return Pattern::Constructor(Tag::Invalid, vec![]);
        }
        Pattern::Constructor(
            tag,
            args.into_iter()
                .zip(fields)
                .map(|(pat, ty)| self.pattern(pat, ty))
                .collect(),
        )
    }

    fn variant_tag(&mut self, path: &ResolvedPath, ty: TypeId) -> Tag {
        if let Some(tag) = self.resolved_variant_tag(path, ty) {
            return tag;
        }
        self.incomplete("pattern is missing a resolved constructor identity for its enum type");
        Tag::Invalid
    }

    fn resolved_variant_tag(&self, path: &ResolvedPath, ty: TypeId) -> Option<Tag> {
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.ctx.symbols.canonical_symbol(self.ctx.hir, symbol)?;
        let symbol = self.ctx.hir.symbols.get(symbol)?;
        let owner = match self.ctx.interner.store().get(ty)? {
            Type::Applied { constructor, .. } => TypeId(constructor.0),
            _ => ty,
        };
        match &symbol.def {
            SymbolDef::EnumVariant {
                enum_item,
                variant_index,
            } => {
                let etas_hir::HirItem::Enum(decl) = self.ctx.hir.items.get(*enum_item)? else {
                    return None;
                };
                let crate::SymbolTypeFact::Type { constructor } =
                    self.ctx.signature_facts.symbol_types.get(&decl.symbol)?
                else {
                    return None;
                };
                (owner == TypeId(constructor.0)).then_some(Tag::Variant {
                    owner,
                    index: *variant_index as usize,
                })
            }
            SymbolDef::ImportAlias { path, .. } => {
                let std = self.ctx.std_registry.lookup_qualified(path)?;
                let fact = self.ctx.signature_facts.symbol_types.get(&symbol.id)?;
                let result = match fact {
                    crate::SymbolTypeFact::Flow { signature } => signature.output,
                    crate::SymbolTypeFact::Value { ty } => *ty,
                    _ => return None,
                };
                let result = match self.ctx.interner.store().get(result)? {
                    Type::Applied { constructor, .. } => TypeId(constructor.0),
                    _ => result,
                };
                if let Some(layout) = self.ctx.signature_facts.enum_layouts.get(&owner) {
                    if result != owner {
                        return None;
                    }
                    let index = layout
                        .variants
                        .iter()
                        .position(|variant| variant.name == std.name)?;
                    return Some(Tag::Variant { owner, index });
                }
                match (
                    self.ctx.interner.store().get(ty)?,
                    self.ctx.interner.store().get(result)?,
                    std.name.as_str(),
                ) {
                    (Type::Option(_), Type::Option(_), "Some") => Some(Tag::Structural("Some")),
                    (Type::Option(_), Type::Option(_), "None") => Some(Tag::Structural("None")),
                    (Type::Result { .. }, Type::Result { .. }, "Ok") => Some(Tag::Structural("Ok")),
                    (Type::Result { .. }, Type::Result { .. }, "Err") => {
                        Some(Tag::Structural("Err"))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    // Pattern-matrix specialization preserves correlations between payload columns.
    fn useful(&mut self, matrix: &[Vec<Pattern>], query: &[Pattern], types: &[TypeId]) -> bool {
        if query.is_empty() {
            return matrix.is_empty();
        }
        if matrix
            .iter()
            .any(|row| row.iter().all(|pat| matches!(pat, Pattern::Any)))
        {
            return false;
        }
        let key = (matrix.to_vec(), query.to_vec(), types.to_vec());
        if !self.active.insert(key.clone()) {
            return true;
        }
        let constructors = self.constructors(types[0]);
        let result = match &query[0] {
            Pattern::Constructor(tag, fields) => {
                let payload_types = if matches!(tag, Tag::Literal(_)) && fields.is_empty() {
                    Vec::new()
                } else if let Some(payload) = constructors
                    .as_ref()
                    .and_then(|cs| cs.iter().find(|c| c.tag == *tag))
                {
                    payload.fields.clone()
                } else {
                    self.incomplete(
                        "pattern matrix constructor is missing its checked payload layout",
                    );
                    self.active.remove(&key);
                    return false;
                };
                let mut next = fields.clone();
                next.extend_from_slice(&query[1..]);
                let mut next_types = payload_types;
                next_types.extend_from_slice(&types[1..]);
                self.useful(&specialize(matrix, tag, fields.len()), &next, &next_types)
            }
            Pattern::Any => match constructors {
                Some(constructors)
                    if constructors.iter().all(|c| {
                        matrix.iter().any(
                            |row| matches!(&row[0], Pattern::Constructor(tag, _) if *tag == c.tag),
                        )
                    }) =>
                {
                    constructors.into_iter().any(|c| {
                        let mut next = vec![Pattern::Any; c.fields.len()];
                        next.extend_from_slice(&query[1..]);
                        let mut next_types = c.fields.clone();
                        next_types.extend_from_slice(&types[1..]);
                        self.useful(
                            &specialize(matrix, &c.tag, c.fields.len()),
                            &next,
                            &next_types,
                        )
                    })
                }
                _ => {
                    let defaults = matrix
                        .iter()
                        .filter(|row| matches!(row[0], Pattern::Any))
                        .map(|row| row[1..].to_vec())
                        .collect::<Vec<_>>();
                    self.useful(&defaults, &query[1..], &types[1..])
                }
            },
        };
        self.active.remove(&key);
        result
    }
}

fn specialize(matrix: &[Vec<Pattern>], tag: &Tag, arity: usize) -> Vec<Vec<Pattern>> {
    matrix
        .iter()
        .filter_map(|row| {
            let mut fields = match &row[0] {
                Pattern::Any => vec![Pattern::Any; arity],
                Pattern::Constructor(candidate, fields) if candidate == tag => fields.clone(),
                _ => return None,
            };
            fields.extend_from_slice(&row[1..]);
            Some(fields)
        })
        .collect()
}
