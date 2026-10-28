use crate::{BodyRelocationLimit, BodyRelocationLimits, BodyRelocationResource, SourceId, Span};

pub(crate) trait SpanNode {
    fn children<'a>(&'a self, work: &mut SpanWork<'a>);
    fn children_mut<'a>(&'a mut self, work: &mut Vec<&'a mut dyn SpanNode>);
    fn span(&self) -> Option<Span> {
        None
    }
    fn set_source(&mut self, _source: SourceId) {}
}

impl SpanNode for Span {
    fn children<'a>(&'a self, _: &mut SpanWork<'a>) {}
    fn children_mut<'a>(&'a mut self, _: &mut Vec<&'a mut dyn SpanNode>) {}
    fn span(&self) -> Option<Span> {
        Some(*self)
    }
    fn set_source(&mut self, source: SourceId) {
        self.source = source;
    }
}

macro_rules! leaf {
    ($($ty:ty),* $(,)?) => { $(
        impl SpanNode for $ty {
            fn children<'a>(&'a self, _: &mut SpanWork<'a>) {}
            fn children_mut<'a>(&'a mut self, _: &mut Vec<&'a mut dyn SpanNode>) {}
        }
    )* };
}
leaf!(String, bool, char);

impl<T: SpanNode> SpanNode for Box<T> {
    fn children<'a>(&'a self, work: &mut SpanWork<'a>) {
        work.push(self.as_ref());
    }
    fn children_mut<'a>(&'a mut self, work: &mut Vec<&'a mut dyn SpanNode>) {
        work.push(self.as_mut());
    }
}
impl<T: SpanNode> SpanNode for Vec<T> {
    fn children<'a>(&'a self, work: &mut SpanWork<'a>) {
        for value in self {
            if !work.push(value) {
                break;
            }
        }
    }
    fn children_mut<'a>(&'a mut self, work: &mut Vec<&'a mut dyn SpanNode>) {
        for value in self {
            work.push(value);
        }
    }
}
impl<T: SpanNode> SpanNode for Option<T> {
    fn children<'a>(&'a self, work: &mut SpanWork<'a>) {
        if let Some(value) = self {
            work.push(value);
        }
    }
    fn children_mut<'a>(&'a mut self, work: &mut Vec<&'a mut dyn SpanNode>) {
        if let Some(value) = self {
            work.push(value);
        }
    }
}

/// Validate before any recursive AST clone. This also catches foreign-source
/// fields buried in type arguments, patterns, handlers, or reflection targets.
pub(crate) fn validate(
    roots: &[&dyn SpanNode],
    source: SourceId,
    source_bytes: usize,
    root_span: Span,
    limits: BodyRelocationLimits,
) -> Result<(), super::RelocatedBodyError> {
    let mut work = SpanWork {
        pending: Vec::new(),
        next_depth: 0,
        nodes: 0,
        limits,
        failure: None,
    };
    for root in roots {
        if !work.push(*root) {
            break;
        }
    }
    while let Some((node, depth)) = work.pending.pop() {
        if let Some(span) = node.span() {
            if span.source != source
                || span.range.start > span.range.end
                || span.range.end.to_usize() > source_bytes
            {
                return Err(super::RelocatedBodyError::InvalidSourceReference);
            }
        }
        work.next_depth = depth + 1;
        node.children(&mut work);
        if work.failure.is_some() {
            break;
        }
    }
    match work.failure {
        Some((resource, limit)) => Err(super::RelocatedBodyError::Resource(BodyRelocationLimit {
            resource,
            limit,
            span: root_span,
        })),
        None => Ok(()),
    }
}

pub(crate) struct SpanWork<'a> {
    pending: Vec<(&'a dyn SpanNode, usize)>,
    next_depth: usize,
    nodes: usize,
    limits: BodyRelocationLimits,
    failure: Option<(BodyRelocationResource, usize)>,
}

impl<'a> SpanWork<'a> {
    pub(crate) fn push(&mut self, node: &'a dyn SpanNode) -> bool {
        if self.failure.is_some() {
            return false;
        }
        let max_depth = self.limits.max_depth.min(crate::MAX_BODY_PARSE_RECURSION);
        if self.nodes >= self.limits.max_nodes {
            self.failure = Some((BodyRelocationResource::AstNodes, self.limits.max_nodes));
        } else if self.next_depth > max_depth {
            self.failure = Some((BodyRelocationResource::Nesting, max_depth));
        } else {
            self.nodes += 1;
            self.pending.push((node, self.next_depth));
        }
        self.failure.is_none()
    }
}

pub(crate) fn relocate(root: &mut dyn SpanNode, source: SourceId) {
    let mut work = vec![root];
    while let Some(node) = work.pop() {
        node.set_source(source);
        node.children_mut(&mut work);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_nested_ast_span_is_relocated_and_joint_node_budget_is_enforced() {
        let source = crate::SourceFile::new(
            SourceId(401),
            None,
            "let value: Option<i32> = Some(42); return match value { Some(number) => number, None => 0, };",
        );
        let parsed = crate::parse_body_contents(source.clone());
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut body = parsed.value;
        let span = body.span;
        let limits = BodyRelocationLimits::default();
        validate(&[&body], source.id, source.text.len(), span, limits).unwrap();
        relocate(&mut body, SourceId(402));
        validate(
            &[&body],
            SourceId(402),
            source.text.len(),
            body.span,
            limits,
        )
        .unwrap();
        assert!(matches!(
            validate(&[&body], source.id, source.text.len(), body.span, limits),
            Err(crate::RelocatedBodyError::InvalidSourceReference)
        ));

        let one_span = Span::new(
            SourceId(402),
            crate::TextRange::new(crate::TextSize(0), crate::TextSize(0)),
        );
        let two_nodes = BodyRelocationLimits {
            max_nodes: 2,
            ..limits
        };
        validate(
            &[&one_span, &one_span],
            SourceId(402),
            0,
            one_span,
            two_nodes,
        )
        .unwrap();
        assert!(matches!(
            validate(
                &[&one_span, &one_span, &one_span],
                SourceId(402),
                0,
                one_span,
                two_nodes
            ),
            Err(crate::RelocatedBodyError::Resource(BodyRelocationLimit {
                resource: BodyRelocationResource::AstNodes,
                ..
            }))
        ));
    }
}
