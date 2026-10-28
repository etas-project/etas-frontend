use std::{path::PathBuf, sync::Arc};

use etas_core::{Diagnostic, LineIndex, SourceFile as CoreSourceFile, SourceId};
use etas_hir::{HirRevision, HirTreeView};
use etas_syntax::{Parse, ast};

use super::module_index::{AstImportRef, AstItemRef};
use crate::{ExternalPackageId, ModulePath};

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct SourceInput {
    pub id: SourceId,
    pub path: Option<PathBuf>,
    pub text: String,
    pub kind: SourceKind,
}

impl SourceInput {
    pub fn anonymous(text: impl Into<String>) -> Self {
        Self {
            id: SourceId(0),
            path: None,
            text: text.into(),
            kind: SourceKind::SingleFileInput,
        }
    }
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct ParseOutput {
    pub source: CoreSourceFile,
    pub parsed: Arc<Parse<ast::Program>>,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct HirOutput {
    pub hir: HirRevision,
}

impl HirOutput {
    pub fn view(&self) -> HirTreeView<'_> {
        self.hir.view()
    }
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct SourceBundle {
    pub sources: Vec<CoreSourceFile>,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct SourceSet {
    pub project_root: PathBuf,
    pub source_root: PathBuf,
    pub environment_fingerprint: String,
    pub external_modules_fingerprint: String,
    pub files: Vec<SourceFile>,
}

impl SourceSet {
    pub fn to_source_bundle(&self) -> SourceBundle {
        SourceBundle {
            sources: self
                .files
                .iter()
                .map(SourceFile::to_core_source_file)
                .collect(),
        }
    }
}

pub(crate) fn source_root_for_project_input(input: &crate::ProjectInput) -> PathBuf {
    input
        .source_root
        .clone()
        .unwrap_or_else(|| input.project_root.clone())
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct SourceFile {
    pub id: SourceId,
    pub path: Option<PathBuf>,
    pub text: Arc<str>,
    pub line_index: LineIndex,
    pub kind: SourceKind,
}

impl SourceFile {
    pub fn from_input(input: &SourceInput) -> Self {
        let text: Arc<str> = Arc::from(input.text.as_str());
        Self {
            id: input.id,
            path: input.path.clone(),
            text: text.clone(),
            line_index: LineIndex::new(&text),
            kind: input.kind.clone(),
        }
    }

    pub fn to_core_source_file(&self) -> CoreSourceFile {
        CoreSourceFile {
            id: self.id,
            path: self.path.clone(),
            text: self.text.clone(),
            line_index: self.line_index.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum SourceKind {
    SourceProjectFile,
    DependencySourceOverlay {
        package: ExternalPackageId,
        import_root: String,
    },
    SingleFileInput,
    VirtualStd,
    VirtualGenerated,
    LspOverlay,
}

impl SourceKind {
    pub fn is_source_project_file(&self) -> bool {
        matches!(
            self,
            Self::SourceProjectFile | Self::DependencySourceOverlay { .. }
        )
    }
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct ParsedSource {
    pub source: SourceId,
    pub source_file: Arc<CoreSourceFile>,
    pub parse: Arc<Parse<ast::Program>>,
    pub declared_module: Option<ModulePath>,
    pub imports: Vec<AstImportRef>,
    pub items: Vec<AstItemRef>,
    pub diagnostics: Vec<Diagnostic>,
}
