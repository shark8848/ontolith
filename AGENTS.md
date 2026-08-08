# AGENTS.md

Repository-scope instructions for coding agents working in this repo.

## Push / remote contract (must-follow)

- `origin` must be the GitHub SSH-over-443 endpoint, because outbound TCP to
  github.com:22 is blocked in this environment (DNS returns a filtered IP and
  the SSH banner exchange times out), while 443 works.
- Canonical remote URL:
  `ssh://git@ssh.github.com:443/shark8848/ontolith.git`
- Do **not** change `origin` back to `git@github.com:shark8848/ontolith.git` —
  a push through port 22 will hang/time out. If `origin` has been reset,
  restore it with:
  ```bash
  git remote set-url origin ssh://git@ssh.github.com:443/shark8848/ontolith.git
  ```
- Verify before pushing when in doubt:
  ```bash
  git remote -v          # must show ssh.github.com:443
  timeout 20 ssh -o ConnectTimeout=10 -T -p 443 git@ssh.github.com 2>&1 | head -3
  # expected: "Hi shark8848! You've successfully authenticated..."
  ```
- The ssh.github.com host key is already in `~/.ssh/known_hosts`.
- Push straight to `main` (direct-push workflow, no PR) as established in
  `docs/PROGRESS.md`.

## Environment notes

- Writable workspace: `/home/ontolith` and `/tmp`; network is restricted in the
  sandbox — commands needing network (git push, cargo fetch, drill runs that
  bind ports) must run with escalation / approved prefixes.
- `scripts/release-rollback-drill.sh` and `scripts/drill-rebalance-dr.sh` are
  staging-only drills (random ports, /tmp data dirs); they must not be pointed
  at production data directories.
