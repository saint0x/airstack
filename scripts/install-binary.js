#!/usr/bin/env node

const { execSync } = require('child_process');
const { existsSync, mkdirSync, copyFileSync, chmodSync } = require('fs');
const { join, dirname } = require('path');
const https = require('https');
const fs = require('fs');

const GITHUB_REPO = 'airstack/airstack';
const VERSION = require('../package.json').version;

function getPlatformInfo() {
  const platform = process.platform;
  const arch = process.arch;
  
  const platformMap = {
    'darwin': 'darwin',
    'linux': 'linux',
    'win32': 'windows',
  };
  
  const archMap = {
    'x64': 'x86_64',
    'arm64': 'arm64',
  };
  
  return {
    platform: platformMap[platform],
    arch: archMap[arch],
    extension: platform === 'win32' ? '.exe' : '',
  };
}

function buildFromSource() {
  console.log('Building Airstack from source...');
  
  try {
    // Check if we have Rust installed
    execSync('cargo --version', { stdio: 'pipe' });
  } catch (error) {
    console.error('❌ Rust/Cargo not found. Please install Rust from https://rustup.rs/');
    process.exit(1);
  }
  
  try {
    console.log('🔨 Compiling Rust binary...');
    execSync('cargo build --release --bin airstack', { 
      stdio: 'inherit',
      cwd: __dirname + '/..',
    });
    
    const { extension } = getPlatformInfo();
    
    // Find the binary in any target directory
    const findResult = execSync('find target -name "airstack" -type f', { 
      encoding: 'utf8',
      cwd: __dirname + '/..'
    }).trim();
    
    if (!findResult) {
      throw new Error('Could not find built airstack binary');
    }
    
    const sourcePath = join(__dirname, '..', findResult.split('\n')[0]);
    const targetDir = join(__dirname, '..', 'bin');
    const targetPath = join(targetDir, `airstack${extension}`);
    
    if (!existsSync(targetDir)) {
      mkdirSync(targetDir, { recursive: true });
    }
    
    copyFileSync(sourcePath, targetPath);
    
    if (process.platform !== 'win32') {
      chmodSync(targetPath, 0o755);
    }
    
    console.log('✅ Successfully built Airstack binary');
    
  } catch (error) {
    console.error('❌ Failed to build from source:', error.message);
    process.exit(1);
  }
}

function downloadBinary() {
  const { platform, arch, extension } = getPlatformInfo();
  
  if (!platform || !arch) {
    console.log('⚠️  Prebuilt binary not available for this platform, building from source...');
    buildFromSource();
    return;
  }
  
  const binaryName = `airstack-${platform}-${arch}${extension}`;
  const downloadUrl = `https://github.com/${GITHUB_REPO}/releases/download/v${VERSION}/${binaryName}`;
  
  const targetDir = join(__dirname, '..', 'bin');
  const targetPath = join(targetDir, `airstack${extension}`);
  
  console.log(`📥 Downloading Airstack binary for ${platform}-${arch}...`);
  console.log(`URL: ${downloadUrl}`);
  
  if (!existsSync(targetDir)) {
    mkdirSync(targetDir, { recursive: true });
  }
  
  const file = fs.createWriteStream(targetPath);
  
  https.get(downloadUrl, (response) => {
    if (response.statusCode === 404) {
      console.log('⚠️  Prebuilt binary not found, building from source...');
      file.close();
      fs.unlinkSync(targetPath);
      buildFromSource();
      return;
    }
    
    if (response.statusCode !== 200) {
      console.error(`❌ Failed to download binary: HTTP ${response.statusCode}`);
      file.close();
      fs.unlinkSync(targetPath);
      process.exit(1);
    }
    
    response.pipe(file);
    
    file.on('finish', () => {
      file.close();
      
      if (process.platform !== 'win32') {
        chmodSync(targetPath, 0o755);
      }
      
      console.log('✅ Successfully downloaded Airstack binary');
    });
    
    file.on('error', (error) => {
      console.error('❌ Failed to write binary:', error.message);
      fs.unlinkSync(targetPath);
      process.exit(1);
    });
    
  }).on('error', (error) => {
    console.log('⚠️  Failed to download, building from source...');
    file.close();
    if (existsSync(targetPath)) {
      fs.unlinkSync(targetPath);
    }
    buildFromSource();
  });
}

function main() {
  console.log('🚀 Installing Airstack binary...');
  
  // Check if binary already exists
  const { extension } = getPlatformInfo();
  const binaryPath = join(__dirname, '..', 'bin', `airstack${extension}`);
  
  if (existsSync(binaryPath)) {
    console.log('✅ Airstack binary already exists');
    return;
  }
  
  // Check if we're in development (has source code)
  const hasSourceCode = existsSync(join(__dirname, '..', 'crates', 'core', 'Cargo.toml'));
  
  if (hasSourceCode) {
    buildFromSource();
  } else {
    downloadBinary();
  }
}

if (require.main === module) {
  main();
}