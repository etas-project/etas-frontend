use std::{ops::Deref, sync::Arc};

use crate::HirProgram;

use super::{HirTreeIndex, HirTreeIndexError, HirTreeView};

#[derive(Clone, Debug)]
pub struct HirRevision {
    program: Arc<HirProgram>,
    index: Arc<HirTreeIndex>,
}

impl HirRevision {
    pub fn try_new(program: HirProgram) -> Result<Self, HirTreeIndexError> {
        Self::from_shared_program(Arc::new(program))
    }

    fn from_shared_program(program: Arc<HirProgram>) -> Result<Self, HirTreeIndexError> {
        let index = Arc::new(HirTreeIndex::try_build(&program)?);
        Ok(Self { program, index })
    }

    pub fn program(&self) -> &HirProgram {
        &self.program
    }

    pub fn shared_program(&self) -> &Arc<HirProgram> {
        &self.program
    }

    pub fn index(&self) -> &HirTreeIndex {
        &self.index
    }

    pub fn shared_index(&self) -> &Arc<HirTreeIndex> {
        &self.index
    }

    pub fn view(&self) -> HirTreeView<'_> {
        HirTreeView::from_revision(self)
    }

    pub fn edit(self) -> HirRevisionEditor {
        HirRevisionEditor {
            program: self.program,
        }
    }
}

impl Deref for HirRevision {
    type Target = HirProgram;

    fn deref(&self) -> &Self::Target {
        self.program()
    }
}

pub struct HirRevisionEditor {
    program: Arc<HirProgram>,
}

impl HirRevisionEditor {
    pub fn program_mut(&mut self) -> &mut HirProgram {
        Arc::make_mut(&mut self.program)
    }

    pub fn finish(self) -> Result<HirRevision, HirTreeIndexError> {
        HirRevision::from_shared_program(self.program)
    }
}
