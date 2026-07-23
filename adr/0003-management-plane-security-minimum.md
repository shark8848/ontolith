# ADR-0003: Management Plane Minimum Security Baseline (TLS-first, OIDC-ready)

- Status: Proposed
- Date: 2026-07-23
- Deciders: sharky-ai
- Tags: security, server, management-plane

## Context

`ontolith-management-server` has been integrated into the platform baseline and now carries control-plane operations (configuration, monitoring, and data-management triggers).

Current controls:

1. Header-based auth mode (`disabled` / `enforced`)
2. Management read/write ACL split (`ONTOLITH_MANAGEMENT_READ_KEY` / `ONTOLITH_MANAGEMENT_WRITE_KEY`)
3. Local/CI smoke gates for `/admin/health` and `runtime_probe`

Gaps:

1. No transport-layer encryption by default
2. No federated identity model (OIDC/JWT)
3. Limited policy granularity for control-plane roles

R1 requires a minimal, implementable hardening path without introducing high integration risk.

## Decision

1. Adopt a TLS-first hardening path for management plane exposure:
   - R1 baseline recommends loopback-only bind for management service unless explicitly exposed.
   - External exposure should be fronted by TLS termination (reverse proxy or ingress).
2. Keep current key-based ACL as immediate control-plane authorization baseline in R1.
3. Prepare OIDC-ready interface constraints for R2:
   - Preserve auth context abstraction in `ontolith-security`.
   - Add an OIDC verifier integration track (token validation + claim mapping) as a follow-up ADR or RFC.
4. Define security gate progression:
   - R1: smoke + ACL + probe + bind posture evidence.
   - R2: TLS mandatory for non-loopback deployments.
   - R2+: OIDC/JWT policy enforcement for management mutations.

## Consequences

### Positive

- Fastest path to reduce management-plane risk in current architecture.
- Maintains compatibility with existing deploy scripts and CI gates.
- Avoids blocking current R1 delivery on identity-provider integration.

### Negative / risks

- Key-based ACL remains weaker than federated identity.
- TLS handling outside process (proxy/ingress) can diverge by environment.
- Role/claim mapping is deferred and requires additional governance.

### Mitigations

- Document strict bind defaults and deployment checklist.
- Add TLS deployment examples in operations docs.
- Track OIDC integration as explicit next milestone item in PROGRESS and PLAN.

## Alternatives considered

| Option | Why not now |
|--------|-------------|
| Immediate in-process TLS + OIDC in R1 | Too much integration and test-surface expansion for current R1 scope |
| OIDC-first without TLS baseline | Leaves transport path weak and increases deployment complexity |
| Keep current state only (no ADR progression) | Security posture remains implicit and difficult to audit |

## References

- `docs/Ontolith_Software_Architecture_Specification.md`
- `docs/Ontolith_Development_Plan.zh-CN.md`
- `docs/Ontolith_Development_Plan.md`
- `docs/PROGRESS.md`
- Related ADRs: `adr/0001-rocksdb-storage-backend.md`, `adr/0002-cluster-mvp-in-process.md`
- Implementation: `crates/ontolith-server/src/management.rs`
