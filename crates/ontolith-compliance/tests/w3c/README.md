# W3C Subset Cases (Phase R1)

This folder hosts a curated subset inspired by W3C SPARQL 1.1 query tests.
It is intentionally small and profile-aligned for R1 gating.

Case classes:

- must-pass: expected to pass now and counted as gate blockers.
- known-gap: executed but currently expected to fail (tracks planned features).
- unsupported: documented but skipped until feature scope expands.

Strict mode:

Set ONTOLITH_W3C_SUBSET_STRICT=1 to require zero xfail and zero in-scope skipped cases.
Out-of-scope unsupported cases can be marked as strict skip-exempt.
