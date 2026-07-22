# RFC-NNNN: <Title>

- Status: Draft | In Review | Accepted | Rejected | Withdrawn
- Date: YYYY-MM-DD
- Authors: <names>
- Reviewers: <names or TBD>
- Tags: <area>
- Related ADRs: <ADR-NNNN or none>

## Summary

One-paragraph statement of the proposal and intended outcome.

## Motivation

Why this change is needed now. Link R1–R4 milestones, PROGRESS items, or
incidents that motivate the work.

## Detailed design

### Goals

- …

### Non-goals

- …

### Design

Describe interfaces, data flow, error semantics, and consistency/security
impact. Prefer trait boundaries over concrete vendor types for Tier A deps.

### Compatibility

- API / on-disk format / protocol impact
- Migration / rollback plan

### Security & multi-tenancy

- Authn/authz surface
- Tenant isolation implications
- Audit / observability hooks

### Observability

- Metrics, logs, traces required for ops

## Alternatives

| Option | Pros | Cons | Why not |
|--------|------|------|---------|
| … | … | … | … |

## Open questions

1. …

## Acceptance criteria

- [ ] Design reviewed
- [ ] ADR written if Tier A dependency or architecture exception
- [ ] Tests / compliance gate listed
- [ ] Docs updated (`docs/PROGRESS.md`, layer IMPL notes)

## References

- PLAN-0001
- SAS-0001
- DEPENDENCY_REGISTER.md
