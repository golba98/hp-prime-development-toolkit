// Isolated, offline install/launch test of locally assembled tarballs.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { platforms } from '../../scripts/release/index.mjs';
const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../..',
);
const option = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i < 0 ? fallback : process.argv[i + 1];
};
const target = option(
  '--target',
  platforms.find(
    ([, os, cpu]) => os === process.platform && cpu === process.arch,
  )?.[0],
);
const binary = path.resolve(
  option(
    '--binary',
    path.join(
      root,
      'target/debug',
      process.platform === 'win32' ? 'hpdev.exe' : 'hpdev',
    ),
  ),
);
const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'hpdev-npm-'));
const npm = (args, cwd) =>
  execFileSync(process.platform === 'win32' ? 'npm.cmd' : 'npm', args, {
    cwd,
    encoding: 'utf8',
    shell: process.platform === 'win32',
    env: { ...process.env, npm_config_cache: path.join(directory, 'cache') },
    maxBuffer: 4 * 1024 * 1024,
  });
try {
  const packed = JSON.parse(
    execFileSync(
      process.execPath,
      [
        path.join(root, 'scripts/release/package.mjs'),
        '--target',
        target,
        '--binary',
        binary,
        '--output',
        path.join(directory, 'dist'),
      ],
      { cwd: root, encoding: 'utf8', maxBuffer: 2 * 1024 * 1024 },
    ),
  );
  const tarballs = path.join(directory, 'tarballs');
  await fs.mkdir(tarballs);
  const pack = (packagePath) => {
    const data = JSON.parse(
      npm(
        ['pack', '--json', '--ignore-scripts', '--pack-destination', tarballs],
        packagePath,
      ),
    );
    if (data[0].files.some((file) => (file.mode & 0o044) !== 0o044))
      throw new Error(
        'Package assets must be readable after a system-wide installation',
      );
    return path.join(tarballs, data[0].filename);
  };
  const platformTarball = pack(packed.platformPackage),
    rootTarball = pack(packed.rootPackage),
    prefix = path.join(directory, 'prefix');
  npm(
    [
      'install',
      '--global',
      '--prefix',
      prefix,
      '--offline',
      '--ignore-scripts',
      '--no-audit',
      '--no-fund',
      platformTarball,
      rootTarball,
    ],
    directory,
  );
  const modules =
    process.platform === 'win32'
      ? path.join(prefix, 'node_modules')
      : path.join(prefix, 'lib/node_modules');
  const launcher = path.join(modules, 'hp-prime-devkit/bin/hpdev.cjs');
  const run = (args) => {
    const result = spawnSync(process.execPath, [launcher, ...args], {
      cwd: directory,
      encoding: 'utf8',
      env: {
        ...process.env,
        PATH: path.dirname(process.execPath),
        PPL_ENGINE: 'firmware',
      },
    });
    if (result.status !== 0)
      throw new Error(
        `Installed launcher failed (${result.status}): ${result.stderr}`,
      );
    return result.stdout;
  };
  if (run(['--version']).trim() !== `hpdev ${packed.version}`)
    throw new Error('incorrect installed version');
  if (!run(['--help']).includes('Usage:'))
    throw new Error('installed help failed');
  const shim =
    process.platform === 'win32'
      ? path.join(prefix, 'hpdev.cmd')
      : path.join(prefix, 'bin/hpdev');
  const shimVersion = execFileSync(shim, ['--version'], {
    cwd: directory,
    encoding: 'utf8',
    shell: process.platform === 'win32',
    env: { ...process.env, PATH: path.dirname(process.execPath) },
  }).trim();
  if (shimVersion !== `hpdev ${packed.version}`)
    throw new Error('installed hpdev command shim failed');
  const example = path.join(modules, 'hp-prime-devkit/examples/SQUARE.ppl');
  const result = JSON.parse(run(['run', example, '--args', '[7]', '--json']));
  if (result.engine !== 'native' || result.return_value.value !== '49')
    throw new Error('installed binary returned incorrect result');
  const native = path.join(
    modules,
    `hp-prime-devkit-${process.platform}-${process.arch}`,
    'bin',
    process.platform === 'win32' ? 'hpdev.exe' : 'hpdev',
  );
  const corrupt = await fs.readFile(native);
  corrupt[0] ^= 1;
  await fs.writeFile(native, corrupt);
  const rejected = spawnSync(
    process.execPath,
    [launcher, '--version', '--json'],
    { cwd: directory, encoding: 'utf8' },
  );
  if (
    rejected.status !== 70 ||
    JSON.parse(rejected.stdout).status !== 'infrastructure_error' ||
    !JSON.parse(rejected.stdout).error.includes('checksum mismatch')
  )
    throw new Error('corrupt executable was not rejected');
  console.log(
    JSON.stringify({
      passed: 5,
      failed: 0,
      platform: `${process.platform}-${process.arch}`,
      version: result.version,
      offline: true,
      outsideCheckout: true,
    }),
  );
} finally {
  await fs.rm(directory, { recursive: true, force: true });
}
