import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { metadata, platforms, resolveRef, prepare, validateBundle, inspectPackage, publish, ensurePublished } from './release.mjs';

const root = path.resolve(import.meta.dirname, '..');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
async function temporary(t) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'hpdev-release-test-'));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  return directory;
}
async function write(directory, file, data) {
  await fs.mkdir(path.dirname(path.join(directory, file)), { recursive: true });
  await fs.writeFile(path.join(directory, file), typeof data === 'object' ? JSON.stringify(data) : data);
}
function archive(directory, name, file) {
  execFileSync('tar', ['-czf', file, '-C', directory, name]);
}
async function fixture(t) {
  const directory = await temporary(t);
  const pkg = await metadata();
  const source = path.join(directory, 'artifacts');
  const output = path.join(directory, 'release');
  const previous = { sha: process.env.RELEASE_SHA, publish: process.env.RELEASE_PUBLISH };
  process.env.RELEASE_SHA = 'a'.repeat(40);
  process.env.RELEASE_PUBLISH = 'true';
  t.after(() => {
    for (const [key, value] of Object.entries({ RELEASE_SHA: previous.sha, RELEASE_PUBLISH: previous.publish })) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
  });
  for (const [index, [target, os, cpu]] of platforms.entries()) {
    const artifact = path.join(source, `hpdev-${target}`);
    await fs.mkdir(artifact, { recursive: true });
    const native = `hpdev-${pkg.version}-${target}`;
    const exe = os === 'win32' ? 'hpdev.exe' : 'hpdev';
    const bytes = `fixture binary: ${target}`;
    const binaryHash = hash(bytes);
    const platform = { name: `hp-prime-devkit-${os}-${cpu}`, version: pkg.version, repository: pkg.repository, os: [os], cpu: [cpu] };
    const platformTree = path.join(directory, `platform-${index}`);
    await write(platformTree, 'package/package.json', platform);
    await write(platformTree, `package/bin/${exe}`, bytes);
    await write(platformTree, 'package/SHA256SUMS', `${binaryHash}  bin/${exe}\n`);
    for (const file of ['LICENSE', 'THIRD_PARTY_NOTICES.md', 'licenses/inventory.json']) await write(platformTree, `package/${file}`, 'notice');
    archive(platformTree, 'package', path.join(artifact, `${platform.name}-${pkg.version}.tgz`));
    const nativeTree = path.join(directory, `native-${index}`);
    for (const file of ['LICENSE', 'THIRD_PARTY_NOTICES.md', 'licenses/inventory.json', 'README.md', 'docs/packaging.md', 'examples/SQUARE.ppl']) await write(nativeTree, `${native}/${file}`, 'fixture');
    await write(nativeTree, `${native}/${exe}`, bytes);
    await write(nativeTree, `${native}/SHA256SUMS`, `${binaryHash}  ${exe}\n`);
    const nativeFile = path.join(artifact, `${native}.tar.gz`);
    archive(nativeTree, native, nativeFile);
    await fs.writeFile(`${nativeFile}.sha256`, `${hash(await fs.readFile(nativeFile))}  ${native}.tar.gz\n`);
    if (index === 0) {
      const launcherTree = path.join(directory, 'launcher');
      await write(launcherTree, 'package/package.json', pkg);
      for (const file of ['LICENSE', 'THIRD_PARTY_NOTICES.md', 'bin/hpdev.cjs', 'examples/SQUARE.ppl', 'docs/packaging.md', 'tests/snapshots/DRAW.png']) await write(launcherTree, `package/${file}`, 'fixture');
      archive(launcherTree, 'package', path.join(artifact, `${pkg.name}-${pkg.version}.tgz`));
    }
  }
  return { source, output, pkg, directory };
}

test('version mismatch and missing platform dependencies are rejected', async t => {
  const directory = await temporary(t);
  const pkg = await metadata();
  await write(directory, 'Cargo.toml', '[workspace.package]\nversion = "0.1.0"\n');
  await write(directory, 'npm/package.json', { ...pkg, version: '9.9.9' });
  await assert.rejects(metadata(directory), /versions must match/);
  await write(directory, 'npm/package.json', { ...pkg, optionalDependencies: {} });
  await assert.rejects(metadata(directory), /five platform dependencies/);
});

test('publish refs require a real stable tag on main and a main workflow', async t => {
  const directory = await temporary(t);
  const git = args => execFileSync('git', args, { cwd: directory, encoding: 'utf8' }).trim();
  git(['init', '-q', '-b', 'main']);
  git(['-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', 'commit', '--allow-empty', '-qm', 'initial']);
  const sha = git(['rev-parse', 'HEAD']);
  git(['update-ref', 'refs/remotes/origin/main', sha]);
  git(['tag', 'v0.1.0']);
  assert.equal(resolveRef('main', false, directory), sha);
  assert.equal(resolveRef('v0.1.0', true, directory, 'refs/heads/main'), sha);
  assert.throws(() => resolveRef('main', true, directory, 'refs/heads/main'), /stable/);
  assert.throws(() => resolveRef('v0.1.0', true, directory, 'refs/heads/feature'), /main workflow/);
  assert.throws(() => resolveRef('v0.2.0', true, directory, 'refs/heads/main'));
  assert.throws(() => resolveRef('--help', false, directory), /Invalid/);
  git(['checkout', '-qb', 'feature']);
  git(['-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', 'commit', '--allow-empty', '-qm', 'outside main']);
  git(['tag', 'v0.2.0']);
  assert.throws(() => resolveRef('v0.2.0', true, directory, 'refs/heads/main'));
});

test('dry-run assembly validates all platforms and preserves package integrity', async t => {
  const { source, output } = await fixture(t);
  const manifest = await prepare(source, output);
  assert.equal(manifest.npm.length, 6);
  assert.equal(manifest.native.length, 5);
  assert.deepEqual(await validateBundle(output), manifest);
  await fs.appendFile(path.join(output, manifest.npm[0].file), 'corruption');
  await assert.rejects(validateBundle(output), /integrity mismatch/);
});

test('missing platform artifacts and incorrect archive checksums fail before publication', async t => {
  const { source, output, pkg } = await fixture(t);
  const target = platforms[4][0];
  const checksum = path.join(source, `hpdev-${target}`, `hpdev-${pkg.version}-${target}.tar.gz.sha256`);
  const original = await fs.readFile(checksum);
  await fs.writeFile(checksum, 'incorrect');
  await assert.rejects(prepare(source, output), /Archive checksum mismatch/);
  await fs.rm(output, { recursive: true });
  await fs.writeFile(checksum, original);
  await fs.rm(path.join(source, `hpdev-${target}`), { recursive: true });
  await assert.rejects(prepare(source, output), /ENOENT/);
});

test('incorrect package identity and platform mapping are rejected', async t => {
  const { source, pkg } = await fixture(t);
  const file = path.join(source, `hpdev-${platforms[0][0]}`, `hp-prime-devkit-linux-x64-${pkg.version}.tgz`);
  assert.throws(() => inspectPackage(file, { name: 'hp-prime-devkit-linux-x64', version: pkg.version, os: 'linux', cpu: 'arm64' }), /Incorrect platform/);
  assert.throws(() => inspectPackage(file, { name: pkg.name, version: pkg.version }), /Incorrect package identity/);
});

test('publication waits for all platforms, publishes launcher last, and safely retries', async t => {
  const { source, output } = await fixture(t);
  const manifest = await prepare(source, output);
  const exposed = new Map();
  const uploads = [];
  const options = {
    lookup: async name => exposed.get(name) || null,
    upload: async file => {
      const entry = manifest.npm.find(entry => entry.file === path.basename(file));
      if (entry.name === 'hp-prime-devkit') assert.equal(exposed.size, 5);
      uploads.push(entry.name);
      exposed.set(entry.name, { dist: { integrity: entry.integrity } });
    },
    wait: async () => {},
  };
  await publish(output, options);
  assert.equal(uploads.length, 6);
  assert.equal(uploads.at(-1), 'hp-prime-devkit');
  await publish(output, options);
  assert.equal(uploads.length, 6, 'retry must not upload existing exact artifacts');
  exposed.set('hp-prime-devkit', { dist: { integrity: 'wrong' } });
  await assert.rejects(publish(output, options), /Existing version differs/);
  assert.equal(uploads.length, 6);
  delete process.env.RELEASE_PUBLISH;
  await assert.rejects(publish(output, options), /requires RELEASE_PUBLISH/);
});

test('unavailable or conflicting registry versions cannot be accepted', async () => {
  const entry = { name: 'hp-prime-devkit', version: '0.1.0', integrity: 'expected' };
  let calls = 0;
  await assert.rejects(ensurePublished(entry, async () => { calls++; return null; }, async () => {}), /Registry did not expose/);
  assert.equal(calls, 12);
  await assert.rejects(ensurePublished(entry, async () => ({ dist: { integrity: 'other' } })), /integrity differs/);
});
