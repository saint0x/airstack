# AGENTS.md — Airstack

## Testing Policy (Strict Fozzy Gate)

- Fozzy is the primary system/regression/prod-readiness gate for this repo.
- Always run strict deterministic checks first:
  - `fozzy doctor --deep --scenario tests/example.fozzy.json --runs 5 --seed 123 --json`
  - `fozzy test --det --strict tests/example.fozzy.json --json`
- Always run host-backed Fozzy scenarios for real CLI/runtime behavior when applicable:
  - `fozzy run tests/fozzy/host_cli_surface.fozzy.json --proc-backend host --fs-backend host --http-backend host --json`
  - `fozzy run tests/fozzy/host_quality_gate.fozzy.json --proc-backend host --fs-backend host --http-backend host --json`
- Always record and validate a real trace before declaring completion:
  - `fozzy run tests/example.fozzy.json --det --seed 4242 --record /tmp/airstack.fozzy --record-collision overwrite --json`
  - `fozzy trace verify /tmp/airstack.fozzy --strict --json`
  - `fozzy replay /tmp/airstack.fozzy --json`
  - `fozzy shrink /tmp/airstack.fozzy --minimize all --budget 10s --json`
  - `fozzy ci /tmp/airstack.fozzy --json`
- Run supporting reporting commands as part of the gate:
  - `fozzy artifacts ls <run-id>`
  - `fozzy report show <run-id> --format pretty`
  - `fozzy env --json`
  - `fozzy usage`

## Completion Rule

- Work is not complete unless Fozzy coverage maps to the actual feature/bug scope being changed.
- If a change affects deploy/runtime/SSH/edge/status flows, add or update a Fozzy scenario that exercises that path.
- Convenience wrapper: `./scripts/fozzy-suite.sh` should pass before sign-off.
