use std::collections::HashSet;

use etas_frontend::{CheckRequest, FrontendSession, ProjectInput, SourceInput};

#[test]
fn body_type_payload_is_local_and_scales_linearly() {
    let mut sizes = Vec::new();
    for count in [16, 32, 64, 128] {
        let mut source = "module main;\n".to_owned();
        for i in 0..count {
            source.push_str(&format!(
                "public flow f_{i}(x: i32) -> i32 {{ let y = x + 0; return y; }}\n"
            ));
        }
        source.push_str("flow main() -> i32 { return f_0(1); }");
        let mut session = FrontendSession::new();
        let project =
            session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        let checked = session
            .check(project, CheckRequest::full_project())
            .unwrap();
        assert!(
            checked.output.checked.is_some(),
            "{:?}",
            checked.diagnostics
        );
        let output = &checked.output;
        let signature = output.signature_types.as_ref().unwrap();
        let types = output.types.as_ref().unwrap();
        let hir = output.hir.as_ref().unwrap();
        let mut exprs = HashSet::new();
        let mut bytes = 0;
        assert_eq!(output.type_body_outputs.len(), count + 1);
        for body in output.type_body_outputs.values() {
            assert!(body.facts.item_signatures.is_empty());
            assert!(body.facts.spec_signatures.is_empty());
            assert!(body.facts.action_signatures.is_empty());
            assert!(body.facts.qualified_action_signatures.is_empty());
            assert!(body.facts.std_spec_impls.is_empty());
            assert!(body.facts.type_param_bounds.is_empty());
            assert!(body.facts.known_std_types.iterables.is_empty());
            assert_eq!(body.base_len, signature.store.len());
            assert!(
                body.types.len() <= 4,
                "only local inference types: {:?}",
                body.types
            );
            for expr in body.facts.expr_types.keys() {
                assert_eq!(hir.tree_index.expr_item[expr], body.owner);
                assert!(
                    exprs.insert(*expr),
                    "expression copied into multiple bodies"
                );
                assert!(types.facts.expr_types.contains_key(expr));
            }
            assert!(body.facts.expr_types.capacity() <= 4 * body.facts.expr_types.len());
            let encoded = bincode::serde::encode_to_vec(body, bincode::config::standard()).unwrap();
            bytes += encoded.len();
            let (decoded, used): (etas_types::BodyTypeOutput, _) =
                bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
            assert_eq!(used, encoded.len());
            assert_eq!(decoded.base, body.base);
            assert_eq!(decoded.types, body.types);
            assert_eq!(decoded.facts.expr_types, body.facts.expr_types);
        }
        assert_eq!(exprs.len(), types.facts.expr_types.len());
        sizes.push(bytes);
    }
    for pair in sizes.windows(2) {
        assert!(
            pair[1] < 3 * pair[0],
            "body type payload grew superlinearly: {sizes:?}"
        );
    }
}
