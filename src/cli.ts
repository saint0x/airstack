#!/usr/bin/env node

import { execSync, spawn } from 'child_process';
import { existsSync } from 'fs';
import { join } from 'path';
import { fileURLToPath } from 'url';
import { dirname } from 'path';
import chalk from 'chalk';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

function getBinaryPath(): string {
  const platform = process.platform;
  const arch = process.arch;
  
  let binaryName = 'airstack';
  if (platform === 'win32') {
    binaryName += '.exe';
  }
  
  // Try different binary locations
  const possiblePaths = [
    join(__dirname, '..', 'bin', `${binaryName}`),
    join(__dirname, '..', 'bin', `${platform}-${arch}`, binaryName),
    join(__dirname, '..', 'target', 'release', binaryName),
    join(__dirname, '..', 'target', 'debug', binaryName),
  ];
  
  for (const path of possiblePaths) {
    if (existsSync(path)) {
      return path;
    }
  }
  
  throw new Error(
    `Airstack binary not found. Looked in:\n${possiblePaths.map(p => `  - ${p}`).join('\n')}\n\n` +
    `This usually means the binary wasn't properly installed. Try:\n` +
    `  npm rebuild airstack\n` +
    `  or reinstall with: npm install -g airstack`
  );
}

function main() {
  try {
    const binaryPath = getBinaryPath();
    const args = process.argv.slice(2);
    
    // Check if we can execute the binary
    try {
      execSync(`"${binaryPath}" --version`, { stdio: 'pipe' });
    } catch (error) {
      console.error(chalk.red('❌ Failed to execute Airstack binary'));
      console.error(chalk.gray(`Binary path: ${binaryPath}`));
      console.error(chalk.gray(`Error: ${error}`));
      process.exit(1);
    }
    
    // Spawn the Rust binary with all arguments
    const child = spawn(binaryPath, args, {
      stdio: 'inherit',
      shell: false,
    });
    
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
    
    // Handle process termination gracefully
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