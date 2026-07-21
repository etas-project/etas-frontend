use std::ops::Range;

pub(crate) fn obsolete_fun_spans(source: &str) -> Vec<Range<usize>> {
    keyword_spans(source, "fun")
}

fn keyword_spans(source: &str, keyword: &str) -> Vec<Range<usize>> {
    source
        .match_indices(keyword)
        .map(|(start, text)| start..start + text.len())
        .filter(|range| is_keyword_boundary(source, range.start, range.end))
        .collect()
}

fn is_keyword_boundary(source: &str, start: usize, end: usize) -> bool {
    let before = source[..start].chars().next_back();
    let after = source[end..].chars().next();

    !before.is_some_and(is_ident_continue) && !after.is_some_and(is_ident_continue)
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}
