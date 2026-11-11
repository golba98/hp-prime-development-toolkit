// Release validation and publication. Only the explicit `publish` command writes to npm.
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const platforms = [
  ['x86_64-unknown-linux-musl', 'linux', 'x64'],
  ['aarch64-unknown-linux-musl', 'linux', 'arm64'],
  ['x86_64-pc-windows-msvc', 'win32', 'x64'],
  ['aarch64-apple-darwin', 'darwin', 'arm64'],
  ['x86_64-apple-darwin', 'darwin', 'x64'],
];
const repository = 'git+https://github.com/golba98/hp-prime-development-toolkit.git';
const registry = 'https://registry.npmjs.org';
const digest = (data, algorithm = 'sha256', encoding = 'hex') => createHash(algorithm).update(data).digest(encoding);
const integrity = data => `sha512-${digest(data, 'sha512', 'base64')}`;
const json = async file => JSON.parse(await fs.readFile(file, 'utf8'));
const run = (command, args, cwd = root) => execFileSync(command, args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], shell: command === 'npm.cmd', maxBuffer: 16 * 1024 * 1024 });
function ensure(condition, message) { if (!condition) throw new Error(message); }
function tar(file, member) { return run('tar', ['-xOf', file, member]); }
function tarBytes(file, member) { return execFileSync('tar', ['-xOf', file, member], { maxBuffer: 128 * 1024 * 1024 }); }
function entries(file) {
  const names = run('tar', ['-tzf', file]).trim().split('\n');
  ensure(names.every(name => !name.startsWith('/') && !name.split('/').includes('..')), `Unsafe archive paths: ${file}`);
  return names;
}
export async function metadata(directory = root) {
  const npm = await json(path.join(directory, 'npm/package.json'));
  const cargo = (await fs.readFile(path.join(directory, 'Cargo.toml'), 'utf8')).match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  ensure(/^\d+\.\d+\.\d+$/.test(cargo), 'A stable Cargo version is required');
  ensure(npm.name === 'hp-prime-devkit' && npm.version === cargo, 'Cargo and npm versions must match');
  const names = platforms.map(([, os, cpu]) => `hp-prime-devkit-${os}-${cpu}`);
  ensure(Object.keys(npm.optionalDependencies).length === names.length && names.every(name => npm.optionalDependencies[name] === cargo), 'All five platform dependencies must match the release version exactly');
  return npm;
}
export function resolveRef(ref, publish, directory = root, workflowRef = process.env.GITHUB_REF) {
  ensure(ref && !ref.startsWith('-') && !/[\r\n]/.test(ref), 'Invalid release ref');
  if (publish) {
    ensure(workflowRef === 'refs/heads/main', 'Publish must be dispatched from the main workflow');
    ensure(/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(ref), 'Publish requires a stable vMAJOR.MINOR.PATCH tag');
  }
  const sha = run('git', ['rev-parse', '--verify', '--end-of-options', `${publish ? `refs/tags/${ref}` : ref}^{commit}`], directory).trim();
  if (publish) run('git', ['merge-base', '--is-ancestor', sha, 'origin/main'], directory);
  return sha;
}
export async function validateRef() {
  const ref = process.env.RELEASE_REF || 'main';
  const publish = process.env.RELEASE_PUBLISH === 'true';
  const sha = resolveRef(ref, publish);
  // Read the selected commit, not whichever branch hosts the dispatched workflow.
  const npm = JSON.parse(run('git', ['show', `${sha}:npm/package.json`]));
  const cargo = run('git', ['show', `${sha}:Cargo.toml`]).match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  ensure(npm.version === cargo && Object.values(npm.optionalDependencies).every(version => version === cargo), 'Selected commit has mismatched package versions');
  if (publish) ensure(ref === `v${cargo}`, 'Tag and package versions must match');
  const result = { sha, version: cargo, tag: publish ? ref : `v${cargo}` };
  if (process.env.GITHUB_OUTPUT) await fs.appendFile(process.env.GITHUB_OUTPUT, Object.entries(result).map(([key, value]) => `${key}=${value}\n`).join(''));
  console.log(JSON.stringify(result));
  return result;
}
export function inspectPackage(file, expected) {
  const names = entries(file);
  const pkg = JSON.parse(tar(file, 'package/package.json'));
  ensure(pkg.name === expected.name && pkg.version === expected.version, `Incorrect package identity: ${file}`);
  ensure(pkg.repository?.url === repository, `Missing repository metadata: ${file}`);
  for (const required of ['LICENSE', 'THIRD_PARTY_NOTICES.md']) ensure(names.includes(`package/${required}`), `Missing ${required}: ${file}`);
  if (expected.os) {
    ensure(JSON.stringify(pkg.os) === JSON.stringify([expected.os]) && JSON.stringify(pkg.cpu) === JSON.stringify([expected.cpu]), `Incorrect platform: ${file}`);
    const executable = expected.os === 'win32' ? 'hpdev.exe' : 'hpdev';
    ensure(names.includes('package/licenses/inventory.json'), `Missing dependency notices: ${file}`);
    const hash = digest(tarBytes(file, `package/bin/${executable}`));
    ensure(tar(file, 'package/SHA256SUMS') === `${hash}  bin/${executable}\n`, `Binary checksum mismatch: ${file}`);
    return hash;
  }
  for (const required of ['bin/hpdev.cjs', 'examples/SQUARE.ppl', 'docs/packaging.md', 'tests/snapshots/DRAW.png']) ensure(names.includes(`package/${required}`), `Missing ${required}: ${file}`);
  ensure(JSON.stringify(pkg.optionalDependencies) === JSON.stringify(expected.optionalDependencies), 'Launcher platform dependencies differ from the release metadata');
}
export async function prepare(source, output) {
  const pkg = await metadata();
  ensure(!process.env.RELEASE_VERSION || pkg.version === process.env.RELEASE_VERSION, 'Unexpected release version');
  await fs.mkdir(output, { recursive: true });
  ensure((await fs.readdir(output)).length === 0, 'Release output directory must be empty');
  const manifest = { version: pkg.version, commit: process.env.RELEASE_SHA || run('git', ['rev-parse', 'HEAD']).trim(), npm: [], native: [] };
  for (const [index, [target, os, cpu]] of platforms.entries()) {
    const directory = path.join(source, `hpdev-${target}`);
    const selected = [{ name: `hp-prime-devkit-${os}-${cpu}`, version: pkg.version, os, cpu }];
    if (index === 0) selected.push(pkg); // The Linux x64 runner supplies the canonical launcher tarball.
    let binaryHash;
    for (const expected of selected) {
      const file = `${expected.name}-${pkg.version}.tgz`;
      const origin = path.join(directory, file);
      await fs.access(origin);
      const hash = inspectPackage(origin, expected);
      if (expected.os) binaryHash = hash;
      const bytes = await fs.readFile(origin);
      await fs.copyFile(origin, path.join(output, file));
      manifest.npm.push({ file, name: expected.name, version: pkg.version, integrity: integrity(bytes), ...(expected.os ? { os, cpu } : {}) });
    }
    const file = `hpdev-${pkg.version}-${target}.tar.gz`;
    const origin = path.join(directory, file);
    const hash = digest(await fs.readFile(origin));
    const checksum = `${hash}  ${file}\n`;
    ensure(await fs.readFile(`${origin}.sha256`, 'utf8') === checksum, `Archive checksum mismatch: ${file}`);
    const names = entries(origin);
    const base = file.slice(0, -7);
    const executable = os === 'win32' ? 'hpdev.exe' : 'hpdev';
    for (const required of [executable, 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'licenses/inventory.json', 'README.md', 'docs/packaging.md', 'examples/SQUARE.ppl']) ensure(names.includes(`${base}/${required}`), `Missing ${required}: ${file}`);
    ensure(digest(tarBytes(origin, `${base}/${executable}`)) === binaryHash, `Native and npm executables differ: ${file}`);
    ensure(tar(origin, `${base}/SHA256SUMS`) === `${binaryHash}  ${executable}\n`, `Native binary checksum mismatch: ${file}`);
    await fs.copyFile(origin, path.join(output, file));
    await fs.writeFile(path.join(output, `${file}.sha256`), checksum);
    manifest.native.push({ file, sha256: hash });
  }
  await fs.writeFile(path.join(output, 'release-manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`Validated ${manifest.npm.length} npm packages and ${manifest.native.length} native archives for ${pkg.version}`);
  return manifest;
}
export async function validateBundle(directory) {
  const manifest = await json(path.join(directory, 'release-manifest.json'));
  const pkg = await metadata();
  ensure(manifest.version === pkg.version && manifest.commit === process.env.RELEASE_SHA, 'Release manifest does not match the selected commit');
  const expectedNames = new Set([pkg.name, ...Object.keys(pkg.optionalDependencies)]);
  ensure(manifest.npm.length === 6 && manifest.native.length === 5, 'Incomplete release manifest');
  for (const entry of manifest.npm) {
    ensure(path.basename(entry.file) === entry.file && entry.file === `${entry.name}-${pkg.version}.tgz` && expectedNames.delete(entry.name), 'Unexpected or duplicate npm artifact');
    const platform = platforms.find(([, os, cpu]) => entry.name === `hp-prime-devkit-${os}-${cpu}`);
    ensure(integrity(await fs.readFile(path.join(directory, entry.file))) === entry.integrity, `Artifact integrity mismatch: ${entry.file}`);
    inspectPackage(path.join(directory, entry.file), platform ? { name: entry.name, version: pkg.version, os: platform[1], cpu: platform[2] } : pkg);
  }
  const expectedArchives = new Set(platforms.map(([target]) => `hpdev-${pkg.version}-${target}.tar.gz`));
  for (const entry of manifest.native) {
    ensure(expectedArchives.delete(entry.file), 'Unexpected or duplicate native artifact');
    ensure(digest(await fs.readFile(path.join(directory, entry.file))) === entry.sha256, `Artifact checksum mismatch: ${entry.file}`);
    ensure(await fs.readFile(path.join(directory, `${entry.file}.sha256`), 'utf8') === `${entry.sha256}  ${entry.file}\n`, 'Archive checksum file differs');
  }
  return manifest;
}
export async function registryVersion(name, version) {
  const response = await fetch(`${registry}/${encodeURIComponent(name)}/${encodeURIComponent(version)}`, { signal: AbortSignal.timeout(30000) });
  if (response.status === 404) return null;
  ensure(response.ok, `Registry lookup failed for ${name}: ${response.status}`);
  return response.json();
}
export async function ensurePublished(entry, lookup = registryVersion, wait = () => new Promise(resolve => setTimeout(resolve, 5000))) {
  for (let attempt = 0; attempt < 12; attempt++) {
    const found = await lookup(entry.name, entry.version);
    if (found) {
      ensure(found.dist?.integrity === entry.integrity, `Published integrity differs for ${entry.name}@${entry.version}; do not overwrite or rebuild this version`);
      return;
    }
    if (attempt < 11) await wait();
  }
  throw new Error(`Registry did not expose ${entry.name}@${entry.version}; rerun the publish job using the preserved artifacts`);
}
export async function publish(directory, {
  lookup = registryVersion,
  upload = file => run('npm', ['publish', file, '--access', 'public', '--tag', 'latest', '--ignore-scripts']),
  wait,
} = {}) {
  ensure(process.env.RELEASE_PUBLISH === 'true', 'Publication requires RELEASE_PUBLISH=true');
  const manifest = await validateBundle(directory);
  // Order is intentional: never publish the launcher before all its dependencies exist.
  const ordered = [...manifest.npm.filter(entry => entry.name !== 'hp-prime-devkit'), manifest.npm.find(entry => entry.name === 'hp-prime-devkit')];
  // Detect collisions across the entire release before uploading anything.
  for (const entry of ordered) {
    const found = await lookup(entry.name, entry.version);
    ensure(!found || found.dist?.integrity === entry.integrity, `Existing version differs: ${entry.name}@${entry.version}`);
  }
  for (const entry of ordered) {
    const found = await lookup(entry.name, entry.version);
    if (found) ensure(found.dist?.integrity === entry.integrity, `Existing version differs: ${entry.name}@${entry.version}`);
    else await upload(path.resolve(directory, entry.file));
    await ensurePublished(entry, lookup, wait);
    console.log(`Verified ${entry.name}@${entry.version}`);
  }
}
export async function complete(directory) {
  ensure(process.env.RELEASE_PUBLISH === 'true', 'Release creation requires RELEASE_PUBLISH=true');
  const manifest = await validateBundle(directory);
  for (const entry of manifest.npm) await ensurePublished(entry);
  const tag = `v${manifest.version}`;
  const api = async endpoint => {
    const response = await fetch(`https://api.github.com/repos/golba98/hp-prime-development-toolkit/${endpoint}`, {
      headers: { Authorization: `Bearer ${process.env.GH_TOKEN}`, Accept: 'application/vnd.github+json' },
      signal: AbortSignal.timeout(30000),
    });
    if (response.status === 404) return null;
    ensure(response.ok, `GitHub API lookup failed: ${response.status}`);
    return response.json();
  };
  let release = await api(`releases/tags/${tag}`);
  if (!release) {
    run('gh', ['release', 'create', tag, '--verify-tag', '--draft', '--generate-notes', '--title', `hpdev ${tag}`]);
    release = await api(`releases/tags/${tag}`);
  }
  ensure(release && !release.prerelease, 'Unexpected GitHub release state');
  const files = [...manifest.native.flatMap(entry => [entry.file, `${entry.file}.sha256`]), 'release-manifest.json'];
  for (const file of files) {
    const existing = release.assets.find(asset => asset.name === file);
    if (existing) {
      const response = await fetch(existing.url, { headers: { Authorization: `Bearer ${process.env.GH_TOKEN}`, Accept: 'application/octet-stream' }, signal: AbortSignal.timeout(60000) });
      ensure(response.ok && digest(Buffer.from(await response.arrayBuffer())) === digest(await fs.readFile(path.join(directory, file))), `Existing release asset differs: ${file}`);
    } else {
      ensure(release.draft, 'Refusing to change an already public release');
      run('gh', ['release', 'upload', tag, path.resolve(directory, file)]);
    }
  }
  if (release.draft) run('gh', ['release', 'edit', tag, '--draft=false']);
  console.log(release.html_url);
}
export async function smoke() {
  const pkg = await metadata();
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'hpdev-public-'));
  try {
    // A private prefix avoids an existing global installation masking the result.
    run(process.platform === 'win32' ? 'npm.cmd' : 'npm', ['install', '--prefix', directory, '--ignore-scripts', '--no-audit', '--no-fund', '--registry', registry, `${pkg.name}@${pkg.version}`]);
    const installed = path.join(directory, 'node_modules', pkg.name);
    const launcher = path.join(installed, 'bin/hpdev.cjs');
    const version = run(process.execPath, [launcher, '--version'], directory).trim();
    ensure(version === `hpdev ${pkg.version}`, 'Public installation returned the wrong version');
    const result = JSON.parse(run(process.execPath, [launcher, 'run', path.join(installed, 'examples/SQUARE.ppl'), '--args', '[7]', '--json'], directory));
    ensure(result.engine === 'native' && result.return_value.value === '49', 'Public installation did not run the native example');
    console.log(`Public installation verified on ${process.platform}/${process.arch}`);
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, source = 'artifacts', output = 'release'] = process.argv.slice(2);
  if (command === 'ref') await validateRef();
  else if (command === 'prepare') await prepare(path.resolve(source), path.resolve(output));
  else if (command === 'verify') await validateBundle(path.resolve(source));
  else if (command === 'publish') await publish(path.resolve(source));
  else if (command === 'complete') await complete(path.resolve(source));
  else if (command === 'smoke') await smoke();
  else throw new Error('Usage: node scripts/release.mjs ref|prepare [ARTIFACTS OUTPUT]|verify DIRECTORY|publish DIRECTORY|smoke');
}
