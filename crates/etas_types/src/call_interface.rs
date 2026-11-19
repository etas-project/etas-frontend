use etas_hir::{HirProgram, SymbolId};

/// Declaration-owned calling information; it is deliberately absent from arrow types.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ParameterInterface {
    pub name: Option<String>,
    pub default: Option<ParameterDefaultRef>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ParameterDefaultRef {
    Source {
        parameter: SymbolId,
    },
    /// The expression and its lexical symbols remain in the defining package.
    External {
        reference: etas_code::descriptor::ParameterDefaultInfo,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct DefaultArgument {
    pub slot: u32,
    pub source: ParameterDefaultRef,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ArgumentBinding {
    /// Parameter positions for supplied arguments, in source evaluation order.
    pub argument_slots: Vec<u32>,
    /// Omitted parameters, in declaration order. These are never language values.
    pub defaults: Vec<DefaultArgument>,
}

impl ArgumentBinding {
    pub fn bind(
        parameters: &[ParameterInterface],
        arguments: &[Option<String>],
    ) -> Result<Self, String> {
        let mut used = vec![false; parameters.len()];
        let mut positional = 0;
        let mut named = false;
        let mut argument_slots = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let slot = match argument {
                Some(name) => {
                    named = true;
                    parameters
                        .iter()
                        .position(|parameter| parameter.name.as_ref() == Some(name))
                        .ok_or_else(|| format!("unknown argument `{name}`"))?
                }
                None => {
                    if named {
                        return Err("positional arguments must precede named arguments".into());
                    }
                    let slot = positional;
                    positional += 1;
                    if slot >= parameters.len() {
                        return Err(format!(
                            "call accepts at most {} argument(s)",
                            parameters.len()
                        ));
                    }
                    slot
                }
            };
            if used[slot] {
                return Err("argument is supplied more than once".into());
            }
            used[slot] = true;
            argument_slots.push(slot as u32);
        }
        let mut defaults = Vec::new();
        for (index, parameter) in parameters.iter().enumerate() {
            if used[index] {
                continue;
            }
            let Some(source) = &parameter.default else {
                return Err(match &parameter.name {
                    Some(name) => format!("missing required argument `{name}`"),
                    None => format!("missing required argument at position {}", index + 1),
                });
            };
            defaults.push(DefaultArgument {
                slot: index as u32,
                source: source.clone(),
            });
        }
        Ok(Self {
            argument_slots,
            defaults,
        })
    }
}

pub(crate) fn source_parameters(
    hir: &HirProgram,
    parameters: &[SymbolId],
) -> Vec<ParameterInterface> {
    parameters
        .iter()
        .map(|parameter| ParameterInterface {
            name: Some(
                hir.symbols
                    .get(*parameter)
                    .expect("declared parameter symbol")
                    .name
                    .clone(),
            ),
            default: hir.parameter_defaults.contains_key(parameter).then_some(
                ParameterDefaultRef::Source {
                    parameter: *parameter,
                },
            ),
        })
        .collect()
}

pub(crate) fn required_parameters(parameters: &[Option<String>]) -> Vec<ParameterInterface> {
    parameters
        .iter()
        .map(|name| ParameterInterface {
            name: name.clone(),
            default: None,
        })
        .collect()
}
