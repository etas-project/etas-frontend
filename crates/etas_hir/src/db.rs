use crate::HirProgram;

#[derive(Clone, Debug, Default)]
pub struct HirDb {
    programs: Vec<HirProgram>,
}

impl HirDb {
    pub fn insert(&mut self, program: HirProgram) -> usize {
        let id = self.programs.len();
        self.programs.push(program);
        id
    }

    pub fn get(&self, id: usize) -> Option<&HirProgram> {
        self.programs.get(id)
    }
}
