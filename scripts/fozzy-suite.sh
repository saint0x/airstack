#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "[fozzy] env"
fozzy env --json

echo "[fozzy] doctor"
fozzy doctor --deep --scenario tests/example.fozzy.json --runs 5 --seed 123 --json

echo "[fozzy] deterministic suite"
fozzy test --det --strict tests/example.fozzy.json --json

echo "[fozzy] host-backed cli surface"
fozzy run tests/fozzy/host_cli_surface.fozzy.json \
  --proc-backend host --fs-backend host --http-backend host --json

echo "[fozzy] host-backed quality gate"
fozzy run tests/fozzy/host_quality_gate.fozzy.json \
  --proc-backend host --fs-backend host --http-backend host --json

echo "[fozzy] one-off run + artifacts/report"
trace_out="/tmp/airstack-fozzy-suite.fozzy"
run_json="$(fozzy run tests/example.fozzy.json --det --seed 4242 --record "$trace_out" --record-collision overwrite --json)"
echo "$run_json"
run_id="$(echo "$run_json" | jq -r '.identity.runId')"

fozzy artifacts ls "$run_id"
fozzy report show "$run_id" --format pretty

if [[ -f "$trace_out" ]]; then
  fozzy trace verify "$trace_out" --strict --json
  fozzy replay "$trace_out" --json
  fozzy shrink "$trace_out" --minimize all --budget 10s --json
  fozzy ci "$trace_out" --json
else
  echo "[fozzy] recorded trace missing at $trace_out"
  exit 1
fi

echo "[fozzy] usage"
fozzy usage

echo "[fozzy] suite complete"
