use std::{collections::BTreeMap, fmt::Write, sync::Arc};

use etas_code::{CodeFingerprint, PackageKey};
use etas_frontend::{
    Frontend, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, NativeDefinitionStore},
        identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};

pub fn prepared(helpers: usize) -> (NativeDefinitionStore, etas_hir::HirExprId) {
    let mut source = "module app; agent Worker(value: i32) -> i32 { return value; }\n".to_owned();
    for helper in 0..helpers {
        writeln!(
            source,
            "flow helper_{helper}(value: i32) -> i32 {{ return value; }}"
        )
        .unwrap();
    }
    source.push_str("flow main() -> unit { let definition = ^Worker; return; }");
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.reflection.retain_bodies = vec!["app.Worker".into()];
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let expression = *checked.checked_reflections().keys().next().unwrap();
    let key = |name| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.reflection-memory",
            name,
        ))
    };
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, key(b"app")).unwrap(),
        standard_package: key(b"std"),
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        Arc::new(DefinitionCatalog::build(checked, &origins, Default::default()).unwrap());
    (
        NativeDefinitionStore::new(
            catalog,
            Arc::new(NativeCodeSyntaxOperations::default()),
            Default::default(),
        ),
        expression,
    )
}
