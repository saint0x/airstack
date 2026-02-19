import { spawn, spawnSync } from 'child_process';
import { existsSync } from 'fs';
import { join } from 'path';
import { fileURLToPath } from 'url';
import { dirname } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

function platformBinaryName(): string {
  return process.platform === 'win32' ? 'airstack.exe' : 'airstack';
}

export function getBinaryCandidates(): string[] {
  const binaryName = platformBinaryName();
  const platform = process.platform;
  const arch = process.arch;

  return [
    join(__dirname, '..', 'bin', binaryName),
    join(__dirname, '..', 'bin', `${platform}-${arch}`, binaryName),
    join(__dirname, '..', 'target', 'release', binaryName),
    join(__dirname, '..', 'target', 'debug', binaryName),
  ];
}

function hasAirstackInPath(): boolean {
  const check = spawnSync('airstack', ['--version'], { stdio: 'ignore' });
  return check.status === 0;
}

export function resolveAirstackBinaryPath(): string {
  const envPath = process.env.AIRSTACK_BINARY;
  if (envPath) {
    if (existsSync(envPath)) {
      return envPath;
    }
    throw new Error(`AIRSTACK_BINARY is set but does not exist: ${envPath}`);
  }

  for (const path of getBinaryCandidates()) {
    if (existsSync(path)) {
      return path;
    }
  }

  if (hasAirstackInPath()) {
    return 'airstack';
  }

  throw new Error(
    `Airstack binary not found. Looked in:\n${getBinaryCandidates()
      .map((path) => `  - ${path}`)
      .join('\n')}\n\n` +
      'This usually means the binary was not properly installed. Try:\n' +
      '  npm rebuild airstack\n' +
      '  or reinstall with: npm install -g airstack'
  );
}

export function assertBinaryRunnable(binaryPath: string): void {
  const check = spawnSync(binaryPath, ['--version'], { stdio: 'ignore' });
  if (check.status !== 0) {
    throw new Error(`Failed to execute Airstack binary at ${binaryPath}`);
  }
}

export function spawnAirstack(binaryPath: string, args: string[]) {
  return spawn(binaryPath, args, {
    stdio: 'inherit',
    shell: false,
  });
}
