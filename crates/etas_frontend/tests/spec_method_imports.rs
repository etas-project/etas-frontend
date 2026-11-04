use etas_frontend::{Frontend, ModulePath, ProjectInput, SourceInput};

#[test]
fn unused_grouped_imports_do_not_change_generic_spec_method_summaries() {
    let helper = include_str!("fixtures/method_resolution/import_layout/helper.es");
    let library = include_str!("fixtures/method_resolution/import_layout/library.es");
    let main = include_str!("fixtures/method_resolution/import_layout/main.es");
    for effectful in [false, true] {
        for grouped in [false, true] {
            for selected in [false, true] {
                for order in [
                    [0, 1, 2],
                    [2, 0, 1],
                    [1, 2, 0],
                    [0, 2, 1],
                    [1, 0, 2],
                    [2, 1, 0],
                ] {
                    let library = if grouped {
                        library.to_owned()
                    } else {
                        library.replace("helper.{First, Second}", "helper.First")
                    };
                    let main = if selected {
                        main.replace("value.read()", "value::Reader.read()")
                    } else {
                        main.to_owned()
                    };
                    let (library, main) = if effectful {
                        (
                            library.replace("![]", "![Console.stdout_write]").replace(
                                "return 21;",
                                "perform Console.stdout_write(\"read\"); return 21;",
                            ),
                            main.replace("![]", "![Console.stdout_write]"),
                        )
                    } else {
                        (library, main)
                    };
                    let mut input = ProjectInput::single_source(SourceInput::anonymous(&main));
                    input.entry.module = Some(ModulePath {
                        segments: vec!["app".into(), "main".into()],
                    });
                    let sources = [
                        ("src/helper.es", helper),
                        ("src/library.es", library.as_str()),
                        ("src/app/main.es", main.as_str()),
                    ];
                    input.sources = order
                        .into_iter()
                        .enumerate()
                        .map(|(index, source)| {
                            let (path, text) = sources[source];
                            let mut source = SourceInput::anonymous(text);
                            source.id = etas_core::SourceId(index as u32);
                            source.path = Some(path.into());
                            source
                        })
                        .collect();
                    let output = Frontend.check_project(input);
                    assert!(
                        output.diagnostics.is_empty(),
                        "effectful={effectful}, grouped={grouped}, selected={selected}, order={order:?}: {:?}",
                        output.diagnostics
                    );
                    let checked = output.checked.unwrap();
                    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
                    let expected = checked
                        .effects
                        .performed_actions
                        .values()
                        .map(|fact| fact.invocation.clone())
                        .collect();
                    assert_eq!(summary.escaping_effects.effects, expected);
                    assert_eq!(summary.requested_actions.effects, expected);
                    assert_eq!(
                        summary.escaping_effects.effects.iter().count(),
                        usize::from(effectful)
                    );
                    assert!(
                        if effectful {
                            matches!(
                                summary.action_trace,
                                etas_effects::ActionTraceDomain::Event(_)
                            )
                        } else {
                            matches!(summary.action_trace, etas_effects::ActionTraceDomain::Empty)
                        },
                        "{:?}",
                        summary.action_trace
                    );
                    assert_eq!(checked.types.checked_method_calls.len(), 1);
                }
            }
        }
    }
}
