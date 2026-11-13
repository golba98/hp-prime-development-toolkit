#!/usr/bin/env node
'use strict';
const { spawn } = require('node:child_process');
const path = require('node:path');
const fs = require('node:fs');
const { createHash } = require('node:crypto');
const version = require('../package.json').version;
const args = process.argv.slice(2);
const platform = `${process.platform}-${process.arch}`;
const supported = new Set([
  'linux-x64',
  'linux-arm64',
  'win32-x64',
  'darwin-arm64',
  'darwin-x64',
]);
function fail(message, code = 70) {
  if (args.includes('--json')) {
    const commands = new Set([
      'check',
      'run',
      'debug',
      'preview',
      'watch',
      'test',
      'trace',
      'doctor',
    ]);
    console.log(
      JSON.stringify({
        schema_version: 1,
        version,
        operation: args.find((arg) => commands.has(arg)) || 'startup',
        engine: 'native',
        file: null,
        source: null,
        status: code === 8 ? 'unsupported' : 'infrastructure_error',
        exit_code: code,
        error: message,
        diagnostics: [],
        compatibility_limitations: [],
        entry: null,
        output: [],
        return_value: null,
        runtime_error: null,
        stack: [],
        artifacts: [],
        duration_ms: 0,
        instructions: 0,
        frames_presented: 0,
        trace: [],
        data: null,
      }),
    );
  } else console.error(`hpdev: ${message}`);
  process.exit(code);
}
if (!supported.has(platform))
  fail(
    `Unsupported platform ${platform}. Build the native CLI from source; see the README.`,
    8,
  );
const packageName = `hp-prime-devkit-${platform}`;
let packageFile;
try {
  packageFile = require.resolve(`${packageName}/package.json`);
} catch {
  fail(
    `Native package ${packageName}@${version} is missing. Reinstall hp-prime-devkit@${version} with optional dependencies enabled (omit --omit=optional / --no-optional). No external installer or download was run.`,
  );
}
let executable;
try {
  const metadata = JSON.parse(fs.readFileSync(packageFile, 'utf8'));
  if (metadata.version !== version)
    fail(
      `Native package version ${metadata.version} differs from launcher ${version}; reinstall the matching version.`,
    );
  executable = path.join(
    path.dirname(packageFile),
    'bin',
    process.platform === 'win32' ? 'hpdev.exe' : 'hpdev',
  );
  if (!fs.existsSync(executable))
    fail(
      `Native executable is missing from ${packageName}@${version}; reinstall the package.`,
    );
  const checksumFile = path.join(path.dirname(packageFile), 'SHA256SUMS');
  if (!fs.existsSync(checksumFile))
    fail(
      `Checksum manifest is missing from ${packageName}@${version}; reinstall the package.`,
    );
  const expected = fs.readFileSync(checksumFile, 'utf8').trim().split(/\s+/)[0];
  if (
    createHash('sha256').update(fs.readFileSync(executable)).digest('hex') !==
    expected
  )
    fail(
      `Native executable checksum mismatch in ${packageName}@${version}; reinstall the exact version.`,
    );
} catch (error) {
  fail(
    `Cannot read ${packageName}@${version}: ${error.message}. Reinstall the exact version.`,
  );
}
const child = spawn(executable, args, { stdio: 'inherit', windowsHide: false });
const handlers = new Map();
for (const signal of ['SIGINT', 'SIGTERM']) {
  const handler = () => child.kill(signal);
  handlers.set(signal, handler);
  process.on(signal, handler);
}
child.on('error', (error) =>
  fail(`Cannot launch ${executable}: ${error.message}`),
);
child.on('exit', (code, signal) => {
  for (const [name, handler] of handlers) process.removeListener(name, handler);
  process.exitCode =
    code ?? (signal === 'SIGINT' ? 130 : signal === 'SIGTERM' ? 143 : 70);
});
