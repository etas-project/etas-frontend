use std::hash::Hash;

pub trait AnalysisUnit: Copy + Ord + Hash {}

impl<T> AnalysisUnit for T where T: Copy + Ord + Hash {}
