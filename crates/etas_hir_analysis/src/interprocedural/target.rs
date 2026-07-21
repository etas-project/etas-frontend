#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallTarget<U> {
    Direct(U),
    Dynamic,
    External,
    Incomplete,
}
