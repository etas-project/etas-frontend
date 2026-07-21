use etas_syntax::ast;

pub fn import_alias_name(import: &ast::ImportDecl) -> Option<String> {
    match &import.tree {
        ast::ImportTree::Single { path, alias, .. } => alias.as_ref().map_or_else(
            || path.segments.last().map(|segment| segment.text.clone()),
            |alias| Some(alias.text.clone()),
        ),
        _ => None,
    }
}
