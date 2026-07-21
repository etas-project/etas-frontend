use etas_utils::JoinSemiLattice;

pub trait AbstractDomain: Clone + JoinSemiLattice {}

impl<T> AbstractDomain for T where T: Clone + JoinSemiLattice {}
