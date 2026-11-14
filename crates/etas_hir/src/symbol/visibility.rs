#[derive(
    Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize, serde::Serialize, serde::Deserialize,
)]
pub enum Visibility {
    Private,
    Public,
}
