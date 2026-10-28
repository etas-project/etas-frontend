use etas_code::descriptor::{
    AnnotationConstructorKind, AnnotationInfo, AnnotationLiteralInfo, AnnotationSequenceKind,
    StaticValueInfo,
};
use etas_package_metadata::{
    AnnotationArgMetadata, AnnotationFieldMetadata, AnnotationMetadata,
    AnnotationValueKind as Kind, AnnotationValueMetadata as Value,
};

use crate::PackageMetadataError;

pub(crate) fn annotation(
    item: Vec<String>,
    annotation: &AnnotationInfo<Vec<String>>,
) -> Result<AnnotationMetadata, PackageMetadataError> {
    Ok(AnnotationMetadata {
        item,
        path: annotation.path.clone(),
        args: annotation
            .arguments
            .iter()
            .map(|arg| {
                Ok(AnnotationArgMetadata {
                    name: arg.name.clone().unwrap_or_default(),
                    value: value(&arg.value)?,
                })
            })
            .collect::<Result<Vec<_>, PackageMetadataError>>()?,
    })
}

fn value(source: &StaticValueInfo<Vec<String>>) -> Result<Value, PackageMetadataError> {
    Ok(match source {
        StaticValueInfo::Literal(literal) => {
            let (kind, value) = match literal {
                AnnotationLiteralInfo::Unit => (Kind::Unit, String::new()),
                AnnotationLiteralInfo::Bool(value) => (Kind::Bool, value.to_string()),
                AnnotationLiteralInfo::Integer(value) => (Kind::Int, value.clone()),
                AnnotationLiteralInfo::Float(value) => (Kind::Float, value.clone()),
                AnnotationLiteralInfo::String(value) => (Kind::String, value.clone()),
                AnnotationLiteralInfo::Character(value) => (Kind::Char, value.to_string()),
            };
            Value {
                kind,
                value,
                ..Default::default()
            }
        }
        StaticValueInfo::Reference(path) => Value {
            kind: Kind::Path,
            path: path.clone(),
            ..Default::default()
        },
        StaticValueInfo::Sequence { kind, elements } => Value {
            kind: match kind {
                AnnotationSequenceKind::Array => Kind::Array,
                AnnotationSequenceKind::List => Kind::List,
                AnnotationSequenceKind::Set => Kind::Set,
                AnnotationSequenceKind::Tuple => Kind::Tuple,
            },
            elements: elements
                .iter()
                .map(|element| value(element))
                .collect::<Result<Vec<_>, _>>()?,
            ..Default::default()
        },
        StaticValueInfo::Record(fields) => Value {
            kind: Kind::Record,
            fields: fields
                .iter()
                .map(|field| {
                    Ok(AnnotationFieldMetadata {
                        name: field.name.clone(),
                        value: value(&field.value)?,
                    })
                })
                .collect::<Result<Vec<_>, PackageMetadataError>>()?,
            ..Default::default()
        },
        StaticValueInfo::Constructor {
            kind,
            target,
            arguments,
        } => {
            let name = target
                .last()
                .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                    reason: "static annotation constructor has an empty canonical path".into(),
                })?;
            let mut elements = Vec::new();
            let mut fields = Vec::new();
            for argument in arguments {
                let value = value(&argument.value)?;
                match &argument.name {
                    Some(name) => fields.push(AnnotationFieldMetadata {
                        name: name.clone(),
                        value,
                    }),
                    None => elements.push(value),
                }
            }
            Value {
                kind: match kind {
                    AnnotationConstructorKind::Limit => Kind::Limit,
                    AnnotationConstructorKind::Trace => Kind::Constructor,
                },
                value: name.clone(),
                path: target.clone(),
                elements,
                fields,
            }
        }
    })
}
