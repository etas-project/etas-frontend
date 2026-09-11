use super::BodyPipelineState;
use crate::{PrimitiveType, Type, TypeId, pipeline::context::TypePipelineContext};
use etas_core::{Diagnostic, Severity, Span, TypeDiagnosticCode};
use etas_hir::{HirLiteral, HirPat, HirPatId};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Pattern {
    Any,
    Constructor(String, Vec<Pattern>),
}

#[derive(Clone)]
struct Constructor {
    tag: String,
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
    };
    let mut matrix = Vec::new();
    for &arm in arms {
        let pattern = coverage.pattern(arm, ty);
        if !coverage.useful(&matrix, std::slice::from_ref(&pattern), &[ty]) {
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
    if coverage.useful(&matrix, &[Pattern::Any], &[ty]) {
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
}

impl Coverage<'_, '_> {
    fn incomplete(&mut self, message: impl Into<String>) {
        self.ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            self.span,
            message,
        ));
    }

    fn constructors(&mut self, ty: TypeId) -> Option<Vec<Constructor>> {
        let ctor = |tag: &str, fields| Constructor {
            tag: tag.into(),
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
                .map(|variant| {
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
                        tag: variant.name,
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
                tag: "record".into(),
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
                        tag: "record".into(),
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
                    HirLiteral::Bool { value, .. } => value.to_string(),
                    HirLiteral::Int { text, .. } => format!("integer:{text}"),
                    HirLiteral::String { value, .. } => format!("string:{value}"),
                    HirLiteral::Char { value, .. } => format!("char:{value}"),
                    HirLiteral::Float { text, .. } => format!("float:{text}"),
                },
                Vec::new(),
            ),
            HirPat::Tuple { elems, .. } => self.positional("tuple".into(), elems, ty),
            HirPat::Variant { path, args, .. } => self.positional(
                path.segments
                    .last()
                    .map(|s| s.name.clone())
                    .unwrap_or_default(),
                args,
                ty,
            ),
            HirPat::Record { path, fields, .. } => {
                let constructors = self.constructors(ty).unwrap_or_default();
                let tag = path
                    .and_then(|path| path.segments.last().map(|s| s.name.clone()))
                    .filter(|name| constructors.iter().any(|c| c.tag == *name))
                    .unwrap_or_else(|| "record".into());
                let Some(constructor) = constructors.into_iter().find(|c| c.tag == tag) else {
                    return Pattern::Constructor(tag, vec![]);
                };
                let patterns = constructor
                    .names
                    .unwrap_or_default()
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
            HirPat::Error { .. } => Pattern::Constructor("invalid".into(), vec![]),
        }
    }

    fn positional(&mut self, tag: String, args: Vec<HirPatId>, ty: TypeId) -> Pattern {
        let fields = self
            .constructors(ty)
            .and_then(|ctors| ctors.into_iter().find(|c| c.tag == tag))
            .map(|c| c.fields)
            .unwrap_or_default();
        Pattern::Constructor(
            tag,
            args.into_iter()
                .zip(fields)
                .map(|(pat, ty)| self.pattern(pat, ty))
                .collect(),
        )
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
                let payload_types = constructors
                    .as_ref()
                    .and_then(|cs| cs.iter().find(|c| c.tag == *tag))
                    .map(|c| c.fields.clone())
                    .unwrap_or_default();
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

fn specialize(matrix: &[Vec<Pattern>], tag: &str, arity: usize) -> Vec<Vec<Pattern>> {
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
