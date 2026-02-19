import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { AirstackClient } from '../dist/sdk.js';

function createFakeBinary() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'airstack-sdk-test-'));
  const binary = path.join(dir, 'airstack');
  const script = `#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "airstack 0.1.0-test"
  exit 0
fi
if [ "$1" = "--config" ]; then
  shift
  shift
fi
if [ "$1" = "--json" ] && [ "$2" = "status" ]; then
  echo '{"project":"test","ok":true}'
  exit 0
fi
echo "unexpected args: $@" >&2
exit 7
`;
  fs.writeFileSync(binary, script, { mode: 0o755 });
  return { dir, binary };
}

test('AirstackClient statusJson parses JSON from wrapped binary', async () => {
  const { dir, binary } = createFakeBinary();
  try {
    const client = new AirstackClient({
      binaryPath: binary,
      configPath: '/tmp/airstack.toml',
    });
    const result = await client.statusJson(true);
    assert.equal(result.project, 'test');
    assert.equal(result.ok, true);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('AirstackClient run returns non-zero failures', async () => {
  const { dir, binary } = createFakeBinary();
  try {
    const client = new AirstackClient({ binaryPath: binary });
    const result = await client.run(['status']);
    assert.equal(result.exitCode, 7);
    assert.match(result.stderr, /unexpected args/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
