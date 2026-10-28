/// Owning-package source retention requests. They are resolved once against
/// checked canonical declarations, never used to expose dependency source.
#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ReflectionOptions {
    pub retain_bodies: Vec<String>,
    pub export_bodies: Vec<String>,
}

impl ReflectionOptions {
    pub(crate) fn canonical_entries(&self) -> (Vec<&str>, Vec<&str>) {
        fn canonical(entries: &[String]) -> Vec<&str> {
            let mut entries = entries.iter().map(String::as_str).collect::<Vec<_>>();
            entries.sort_unstable();
            entries.dedup();
            entries
        }
        (
            canonical(&self.retain_bodies),
            canonical(&self.export_bodies),
        )
    }
}
