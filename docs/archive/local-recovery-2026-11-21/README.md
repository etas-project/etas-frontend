# Recovered Design And Audit History

These local materials were recovered during the 2026-11-21 synchronization with
`main`. They are historical records, not a current engineering backlog or proof
that the described features remain unimplemented. Several features were already
integrated and then changed upstream.

The additional `tracked-design-deltas.patch.json` preserves older local edits to
existing architecture documents against the Git base recorded in each section.
It is historical evidence, not a patch to apply to current architecture.

## Designs And Historical Worklists

- [Agent step](architect/agent-step-implementation-worklist.md)
- [Checked reflection](architect/etas-reflection-design.md)
- [Method resolution](architect/method-resolution-design.md)
- [Method migration worklist](architect/method-resolution-implementation-worklist.md)
- [Reflection worklist](architect/reflection-implementation-worklist.md)
- [Residual checks worklist](architect/residual-checks-implementation-worklist.md)
- [Text API worklist](architect/text-api-implementation-worklist.md)

## Dated Audit Evidence

- [Compiler memory audit, 2026-10-23](review/compiler-memory-audit-2026-10-23.md)
- [Original machine-readable measurements](review/compiler-memory-audit-2026-10-23.json)

Do not present the old measurements as current performance or use a worklist
as an outstanding task assignment. The current SPEC and implementation take
precedence. Original references and paths are preserved and may be obsolete.
The immutable original snapshots and checksums remain in the private workspace
backup at `_backups/20261121T113929Z-before-main-sync/`.
