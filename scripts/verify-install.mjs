// Execute the installed artifacts outside the checkout. No publication happens here.
import fs from 'node:fs/promises';
import path from 'node:path';
import hostOS from 'node:os';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { platforms, legacyPlatforms, isMuslBundle, metadata, validateBundle, inspectPackage, inspectNative, nativeArchiveName } from './release.mjs';

const registry = 'https://registry.npmjs.org';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
function ensure(value, message) { if (!value) throw new Error(message); }
function currentPlatform(target = process.env.RELEASE_TARGET) {
  const platform = [...platforms, ...legacyPlatforms].find(([name, os, cpu]) => target ? name === target : os === process.platform && cpu === process.arch);
  ensure(platform && platform[1] === process.platform && platform[2] === process.arch, `Wrong or unsupported runner: ${target} on ${process.platform}/${process.arch}`);
  return platform;
}
function run(command, args, cwd, env) {
  return execFileSync(command, args, { cwd, env: { ...process.env, ...env }, encoding: 'utf8', shell: command.endsWith('.cmd'), maxBuffer: 8 * 1024 * 1024 });
}
function checkCommands(command, version, example, cwd, env) {
  ensure(run(command, ['--version'], cwd, env).trim() === `hpdev ${version}`, 'Installed command returned the wrong version');
  ensure(run(command, ['--help'], cwd, env).includes('Usage:'), 'Installed --help failed');
  const result = JSON.parse(run(command, ['run', example, '--args', '[7]', '--json'], cwd, env));
  ensure(result.engine === 'native' && result.return_value?.value === '49', 'Installed SQUARE example failed');
}
export async function verifyNpm(bundle) {
  const manifest = await validateBundle(bundle);
  const [, platform, cpu] = currentPlatform();
  const name = `hp-prime-devkit-${platform}-${cpu}`;
  const directory = await fs.mkdtemp(path.join(hostOS.tmpdir(), 'hpdev-public-npm-'));
  try {
    run(process.platform === 'win32' ? 'npm.cmd' : 'npm', ['install', '--global', '--prefix', directory,
      '--cache', path.join(directory, 'cache'), '--ignore-scripts', '--no-audit', '--no-fund', '--registry', registry,
      `hp-prime-devkit@${manifest.version}`], directory);
    const modules = path.join(directory, ...(process.platform === 'win32' ? ['node_modules'] : ['lib', 'node_modules']));
    const installed = path.join(modules, 'hp-prime-devkit');
    const selected = createRequire(path.join(installed, 'bin/hpdev.cjs')).resolve(`${name}/package.json`);
    const pkg = JSON.parse(await fs.readFile(selected, 'utf8'));
    ensure(pkg.name === name && pkg.version === manifest.version && pkg.os?.[0] === platform && pkg.cpu?.[0] === cpu, 'Wrong native optional dependency selected');
    const expected = manifest.npm.find(entry => entry.name === name);
    const hash = inspectPackage(path.join(bundle, expected.file), { name, version: manifest.version, os: platform, cpu, legacyMusl: isMuslBundle(manifest) });
    const executable = path.join(path.dirname(selected), 'bin', platform === 'win32' ? 'hpdev.exe' : 'hpdev');
    ensure(sha256(await fs.readFile(executable)) === hash, 'Public native binary differs from the reviewed package');
    const shim = path.join(directory, ...(process.platform === 'win32' ? ['hpdev.cmd'] : ['bin', 'hpdev']));
    checkCommands(shim, manifest.version, path.join(installed, 'examples/SQUARE.ppl'), directory,
      { PATH: path.dirname(process.execPath), PPL_ENGINE: 'firmware' });
    console.log(`PASS npm installation: ${process.platform}/${process.arch}; selected ${name}; version, help and SQUARE verified`);
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
}
async function executeArchive(file, { target, version, sha256: expectedHash, binaryHash }) {
  const [,, cpu] = currentPlatform(target);
  const platform = process.platform;
  ensure(sha256(await fs.readFile(file)) === expectedHash, 'Downloaded native archive checksum differs');
  inspectNative(file, version, target, platform, binaryHash);
  const directory = await fs.mkdtemp(path.join(hostOS.tmpdir(), 'hpdev-native-install-'));
  try {
    if(file.endsWith('.zip')) run(process.env.PYTHON || (process.platform==='win32'?'python':'python3'),
      [fileURLToPath(new URL('./native_zip.py',import.meta.url)),'extract',file,directory],directory);
    else run('tar', ['-xzf', file, '-C', directory], directory);
    const base = path.join(directory, `hpdev-${version}-${target}`);
    const executable = path.join(base, platform === 'win32' ? 'hpdev.exe' : 'hpdev');
    ensure(sha256(await fs.readFile(executable)) === binaryHash, 'Extracted executable checksum differs');
    // PATH contains no Rust, Node, npm or checkout binaries.
    checkCommands(executable, version, path.join(base, 'examples/SQUARE.ppl'), directory,
      { PATH: directory, PPL_ENGINE: 'firmware' });
    console.log(`PASS standalone archive: ${platform}/${cpu}; version, help and SQUARE verified without Node/Rust on PATH`);
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
}
export async function verifyLocalNative(directory, target) {
  currentPlatform(target);
  const pkg = await metadata();
  const [, os, cpu] = currentPlatform(target);
  const file = path.resolve(directory, nativeArchiveName(pkg.version,target));
  const checksum = await fs.readFile(file + '.sha256', 'utf8');
  const hash = sha256(await fs.readFile(file));
  ensure(checksum === `${hash}  ${path.basename(file)}\n`, 'Local archive checksum differs');
  const name = `hp-prime-devkit-${os}-${cpu}`;
  const binaryHash = inspectPackage(path.resolve(directory, `${name}-${pkg.version}.tgz`), { name, version: pkg.version, os, cpu });
  await executeArchive(file, { target, version: pkg.version, sha256: hash, binaryHash });
}
export async function verifyNative(bundle, { publicRelease = false, download } = {}) {
  const manifest = await validateBundle(bundle);
  const [target, os, cpu] = currentPlatform();
  const entry = manifest.native.find(entry => entry.file === nativeArchiveName(manifest.version,target,isMuslBundle(manifest)));
  const name = `hp-prime-devkit-${os}-${cpu}`;
  const npm = manifest.npm.find(entry => entry.name === name);
  const binaryHash = inspectPackage(path.join(bundle, npm.file), { name, version: manifest.version, os, cpu, legacyMusl: isMuslBundle(manifest) });
  const directory = await fs.mkdtemp(path.join(hostOS.tmpdir(), 'hpdev-release-download-'));
  try {
    if (download) await download(directory, entry.file);
    else if (publicRelease) {
      // Public browser download URLs require no GitHub token.
      for (const file of [entry.file, entry.file + '.sha256']) {
        const response = await fetch(`https://github.com/golba98/hp-prime-development-toolkit/releases/download/v${manifest.version}/${file}`, { signal: AbortSignal.timeout(120000) });
        ensure(response.ok, `Public GitHub download failed: ${file} (${response.status})`);
        await fs.writeFile(path.join(directory, file), Buffer.from(await response.arrayBuffer()));
      }
    } else {
      run('gh', ['release', 'download', `v${manifest.version}`, '--repo', 'golba98/hp-prime-development-toolkit',
        '--dir', directory, '--pattern', entry.file, '--pattern', entry.file + '.sha256'], directory);
    }
    ensure(await fs.readFile(path.join(directory, entry.file + '.sha256'), 'utf8') === `${entry.sha256}  ${entry.file}\n`, 'Downloaded checksum file differs');
    await executeArchive(path.join(directory, entry.file), { target, version: manifest.version, sha256: entry.sha256, binaryHash });
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, directory, target] = process.argv.slice(2);
  if (command !== 'local') throw new Error('Usage: node scripts/verify-install.mjs local DIST TARGET');
  await verifyLocalNative(path.resolve(directory), target);
}
