use etas_core::Span;

use crate::{HirTypeId, ResolvedPath};

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct HirReflectExpr {
    pub target: HirReflectTarget,
    pub span: Span,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub enum HirReflectTarget {
    Path(ResolvedPath),
    SpecMethod {
        receiver: ResolvedPath,
        spec: ResolvedPath,
        spec_args: Vec<HirTypeId>,
        method: String,
        method_span: Span,
    },
}

impl HirReflectTarget {
    pub fn root_path(&self) -> &ResolvedPath {
        match self {
            Self::Path(path) => path,
            Self::SpecMethod { receiver, .. } => receiver,
        }
    }

    pub fn paths(&self) -> impl Iterator<Item = &ResolvedPath> {
        [
            Some(self.root_path()),
            match self {
                Self::SpecMethod { spec, .. } => Some(spec),
                Self::Path(_) => None,
            },
        ]
        .into_iter()
        .flatten()
    }

    pub fn path_mut(&mut self, index: usize) -> Option<&mut ResolvedPath> {
        match (self, index) {
            (Self::Path(path), 0) | (Self::SpecMethod { receiver: path, .. }, 0) => Some(path),
            (Self::SpecMethod { spec, .. }, 1) => Some(spec),
            _ => None,
        }
    }
}
