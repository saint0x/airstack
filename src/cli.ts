#!/usr/bin/env node

import chalk from 'chalk';
import { assertBinaryRunnable, resolveAirstackBinaryPath, spawnAirstack } from './binary.js';

function main() {
  try {
    const binaryPath = resolveAirstackBinaryPath();
    const args = process.argv.slice(2);

    assertBinaryRunnable(binaryPath);
    const child = spawnAirstack(binaryPath, args);

    child.on('error', (error) => {
      console.error(chalk.red('❌ Failed to start Airstack:'), error.message);
      process.exit(1);
    });

    child.on('exit', (code, signal) => {
      if (signal) {
        console.error(chalk.yellow(`⚠️  Airstack was terminated by signal: ${signal}`));
        process.exit(1);
      } else if (code !== null) {
        process.exit(code);
      }
    });

    process.on('SIGINT', () => {
      child.kill('SIGINT');
    });

    process.on('SIGTERM', () => {
      child.kill('SIGTERM');
    });

  } catch (error) {
    console.error(chalk.red('❌ Airstack CLI Error:'));
    console.error(chalk.gray((error as Error).message));
    console.error();
    console.error(chalk.blue('💡 Troubleshooting:'));
    console.error(chalk.gray('  1. Make sure you have the required system dependencies'));
    console.error(chalk.gray('  2. Try reinstalling: npm install -g airstack'));
    console.error(chalk.gray('  3. Check the GitHub issues: https://github.com/airstack/airstack/issues'));
    process.exit(1);
  }
}

main();
