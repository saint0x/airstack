import { execa } from 'execa';
import { resolveAirstackBinaryPath } from './binary.js';

export interface AirstackCommandResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export interface AirstackClientOptions {
  binaryPath?: string;
  configPath?: string;
  cwd?: string;
  timeoutMs?: number;
  env?: NodeJS.ProcessEnv;
}

export class AirstackClient {
  private readonly binaryPath: string;
  private readonly configPath?: string;
  private readonly cwd?: string;
  private readonly timeoutMs?: number;
  private readonly env?: NodeJS.ProcessEnv;

  constructor(options: AirstackClientOptions = {}) {
    this.binaryPath = options.binaryPath ?? resolveAirstackBinaryPath();
    this.configPath = options.configPath;
    this.cwd = options.cwd;
    this.timeoutMs = options.timeoutMs;
    this.env = options.env;
  }

  async run(args: string[]): Promise<AirstackCommandResult> {
    const result = await execa(this.binaryPath, this.withGlobalArgs(args), {
      all: false,
      cwd: this.cwd,
      timeout: this.timeoutMs,
      env: this.env,
      reject: false,
    });

    return {
      stdout: result.stdout,
      stderr: result.stderr,
      exitCode: result.exitCode ?? 1,
    };
  }

  async runJson<T>(args: string[]): Promise<T> {
    const result = await this.run(['--json', ...args]);
    if (result.exitCode !== 0) {
      throw new Error(
        `Airstack command failed with exit ${result.exitCode}: ${result.stderr || result.stdout}`
      );
    }

    try {
      return JSON.parse(result.stdout) as T;
    } catch (error) {
      throw new Error(
        `Failed to parse JSON output from Airstack: ${(error as Error).message}\nOutput: ${result.stdout}`
      );
    }
  }

  async statusJson<T = unknown>(detailed = false): Promise<T> {
    const args = ['status'];
    if (detailed) {
      args.push('--detailed');
    }
    return this.runJson<T>(args);
  }

  async ssh(server: string, command: string[]): Promise<AirstackCommandResult> {
    return this.run(['ssh', server, '--', ...command]);
  }

  async cexec(server: string, container: string, command: string[]): Promise<AirstackCommandResult> {
    return this.run(['cexec', server, container, '--', ...command]);
  }

  private withGlobalArgs(args: string[]): string[] {
    if (!this.configPath) {
      return args;
    }
    return ['--config', this.configPath, ...args];
  }
}
