import test from 'node:test';

import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { gzipSync } from 'node:zlib';
import { metadata, platforms, resolveRef, prepare, validateBundle, inspectPackage, publish, ensurePublished, githubRelease, validateMetadata, validateRef } from './release.mjs';

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
  for (const [target] of platforms.slice(1)) await fs.copyFile(
    path.join(source, `hpdev-${platforms[0][0]}`, `${pkg.name}-${pkg.version}.tgz`),
    path.join(source, `hpdev-${target}`, `${pkg.name}-${pkg.version}.tgz`));
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

test('publish refs require a real stable tag on main and the matching tag workflow', async t => {
  const directory = await temporary(t);
  const git = args => execFileSync('git', args, { cwd: directory, encoding: 'utf8' }).trim();
  git(['init', '-q', '-b', 'main']);
  git(['-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', 'commit', '--allow-empty', '-qm', 'initial']);
  const sha = git(['rev-parse', 'HEAD']);
  git(['update-ref', 'refs/remotes/origin/main', sha]);
  git(['tag', 'v0.1.0']);
  assert.equal(resolveRef('main', false, directory), sha);
  assert.equal(resolveRef('v0.1.0', true, directory, 'refs/tags/v0.1.0'), sha);
  assert.throws(() => resolveRef('main', true, directory, 'refs/heads/main'), /stable/);
  assert.throws(() => resolveRef('v0.1.0', true, directory, 'refs/heads/feature'), /selected version tag/);
  assert.throws(() => resolveRef('v0.2.0', true, directory, 'refs/tags/v0.2.0'));
  assert.throws(() => resolveRef('--help', false, directory), /Invalid/);
  git(['checkout', '-qb', 'feature']);
  git(['-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', 'commit', '--allow-empty', '-qm', 'outside main']);
  git(['tag', 'v0.2.0']);
  assert.throws(() => resolveRef('v0.2.0', true, directory, 'refs/tags/v0.2.0'));
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
  await assert.rejects(prepare(source, output), /Missing, duplicate or unexpected/);
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

test('artifact collection rejects extra files, duplicate members and conflicting launchers', async t => {
  const { source, output, pkg, directory } = await fixture(t);
  const artifact = path.join(source, `hpdev-${platforms[0][0]}`);
  await fs.writeFile(path.join(artifact, 'unexpected.tgz'), 'extra');
  await assert.rejects(prepare(source, output), /unexpected files/);
  await fs.rm(path.join(artifact, 'unexpected.tgz'));
  await fs.rm(output, { recursive: true, force: true });
  const launcher = path.join(source, `hpdev-${platforms[1][0]}`, `${pkg.name}-${pkg.version}.tgz`);
  const altered = path.join(directory, 'altered-launcher');
  execFileSync('tar', ['-xzf', launcher, '-C', directory]);
  await fs.rename(path.join(directory, 'package'), altered);
  await fs.mkdir(path.join(directory, 'conflict'));
  await fs.rename(altered, path.join(directory, 'conflict/package'));
  await write(path.join(directory, 'conflict'), 'package/README.md', 'changed by a runner');
  archive(path.join(directory, 'conflict'), 'package', launcher);
  await assert.rejects(prepare(source, output), /Conflicting launcher/);
  const duplicate = path.join(directory, 'duplicate.tar');
  execFileSync('tar', ['-cf', duplicate, '-C', path.join(directory, 'conflict'), 'package/package.json']);
  execFileSync('tar', ['-rf', duplicate, '-C', path.join(directory, 'conflict'), 'package/package.json']);
  await fs.writeFile(duplicate + '.gz', gzipSync(await fs.readFile(duplicate)));
  assert.throws(() => inspectPackage(duplicate + '.gz', pkg), /Duplicate archive members/);
});

test('bundle verification rejects duplicate manifest entries and extra files', async t => {
  const { source, output } = await fixture(t);
  const manifest = await prepare(source, output);
  await fs.writeFile(path.join(output, 'extra.tgz'), 'extra');
  await assert.rejects(validateBundle(output), /Unexpected or duplicate release bundle/);
  await fs.rm(path.join(output, 'extra.tgz'));
  const changed = structuredClone(manifest);
  changed.native[4] = changed.native[0];
  await write(output, 'release-manifest.json', changed);
  await assert.rejects(validateBundle(output), /duplicate native artifact/);
  changed.native = manifest.native;
  changed.npm[5] = changed.npm[0];
  await write(output, 'release-manifest.json', changed);
  await assert.rejects(validateBundle(output), /duplicate npm artifact/);
});

test('GitHub draft upload, retry, exact assets and public finalization are separate', async t => {
  const { source, output } = await fixture(t);
  const manifest = await prepare(source, output);
  const content = new Map();
  const commands = [];
  let release = null;
  const options = {
    api: async () => release ? [release] : [],
    lookup: async name => ({ dist: { integrity: manifest.npm.find(entry => entry.name === name).integrity } }),
    tagCommit: () => manifest.commit,
    download: async asset => content.get(asset.name),
    command: (command, args) => {
      commands.push(args);
      if (args[1] === 'create') release = { tag_name: 'v0.1.0', draft: true, prerelease: false, assets: [], html_url: 'https://example.invalid/release' };
      if (args[1] === 'upload') {
        const name = path.basename(args[3]);
        content.set(name, execFileSync(process.execPath, ['-e', 'process.stdout.write(require("fs").readFileSync(process.argv[1]))', args[3]]));
        release.assets.push({ name });
      }
      if (args[1] === 'edit') release.draft = false;
    },
  };
  await assert.rejects(githubRelease(output, true, options), /Verify draft archive installation/);
  await githubRelease(output, false, options);
  assert.equal(release.draft, true);
  assert.equal(release.assets.length, 12);
  const count = commands.length;
  await githubRelease(output, false, options);
  assert.equal(commands.length, count, 'retry must not overwrite assets');
  content.set(release.assets[0].name, Buffer.from('conflict'));
  await assert.rejects(githubRelease(output, true, options), /Existing release asset differs/);
  content.set(release.assets[0].name, await fs.readFile(path.join(output, release.assets[0].name)));
  await githubRelease(output, true, options);
  assert.equal(release.draft, false);
  await githubRelease(output, true, options);
  assert.equal(commands.filter(args => args[1] === 'edit').length, 1);
  await assert.rejects(githubRelease(output, false, { ...options, tagCommit: () => 'b'.repeat(40) }), /tag moved/);
  release.assets.push(release.assets[0]);
  await assert.rejects(githubRelease(output, false, options), /duplicate release assets/);
});

test('preflight rejects malformed workspace versions and wrong optional dependency names', async () => {
  const pkg = await metadata();
  assert.throws(() => validateMetadata('[package]\nversion = "0.1.0"', pkg), /stable Cargo workspace/);
  assert.throws(() => validateMetadata('[workspace.package]\nversion = "00.1.0"', pkg), /stable Cargo workspace/);
  const optionalDependencies = { ...pkg.optionalDependencies };
  delete optionalDependencies['hp-prime-devkit-linux-x64'];
  optionalDependencies['hp-prime-devkit-wrong'] = '0.1.0';
  assert.throws(() => validateMetadata('[workspace.package]\nversion = "0.1.0"', { ...pkg, optionalDependencies }), /five platform dependencies/);
});

test('tag validation reads the selected commit and checks dry-run tags too', async t => {
  const directory = await temporary(t);
  const pkg = await metadata();
  const git = args => execFileSync('git', args, { cwd: directory, encoding: 'utf8' }).trim();
  git(['init', '-q', '-b', 'main']);
  await write(directory, 'npm/package.json', pkg);
  await write(directory, 'Cargo.toml', '[workspace.package]\nversion = "0.1.0"\n');
  git(['add', '.']);
  git(['-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', 'commit', '-qm', 'metadata']);
  git(['tag', 'v9.9.9']);
  const previous = Object.fromEntries(['RELEASE_REF', 'RELEASE_PUBLISH', 'GITHUB_REF', 'GITHUB_SHA'].map(key => [key, process.env[key]]));
  t.after(() => { for (const [key, value] of Object.entries(previous)) if (value === undefined) delete process.env[key]; else process.env[key] = value; });
  process.env.RELEASE_REF = 'v9.9.9';
  process.env.RELEASE_PUBLISH = 'false';
  await assert.rejects(validateRef(directory), /Tag and package versions/);
  process.env.RELEASE_REF = 'main';
  assert.equal((await validateRef(directory)).version, '0.1.0');
  const sha = git(['rev-parse', 'HEAD']);
  git(['update-ref', 'refs/remotes/origin/main', sha]);
  git(['tag', 'v0.1.0']);
  process.env.RELEASE_REF = 'v0.1.0';
  process.env.RELEASE_PUBLISH = 'true';
  process.env.GITHUB_REF = 'refs/tags/v0.1.0';
  process.env.GITHUB_SHA = sha;
  assert.equal((await validateRef(directory)).sha, sha);
  process.env.GITHUB_SHA = 'b'.repeat(40);
  await assert.rejects(validateRef(directory), /Workflow provenance commit differs/);
});

test('downloaded GitHub checksums and wrong runners fail explicitly', async t => {
  const { verifyNative } = await import('./verify-install.mjs');
  const { source, output } = await fixture(t);
  const manifest = await prepare(source, output);
  const previous = process.env.RELEASE_TARGET;
  t.after(() => { if (previous === undefined) delete process.env.RELEASE_TARGET; else process.env.RELEASE_TARGET = previous; });
  process.env.RELEASE_TARGET = 'unsupported';
  await assert.rejects(verifyNative(output), /Wrong or unsupported runner/);
  const [target] = platforms.find(([, os, cpu]) => os === process.platform && cpu === process.arch);
  process.env.RELEASE_TARGET = target;
  await assert.rejects(verifyNative(output, { download: async (directory, file) => {
    await fs.copyFile(path.join(output, file), path.join(directory, file));
    await fs.writeFile(path.join(directory, file + '.sha256'), 'wrong checksum');
  } }), /Downloaded checksum file differs/);
  assert.equal(manifest.native.length, 5);
});

test('reused bundles require the exact tag workflow identity, not self-reported metadata', async () => {
  const { validateSourceRun } = await import('./release.mjs');
  const commit = 'a'.repeat(40), tag = 'v0.1.0';
  const record = { name: 'Manual release', path: '.github/workflows/release.yml', event: 'workflow_dispatch',
    conclusion: 'success', head_sha: commit, head_repository: { full_name: 'golba98/hp-prime-development-toolkit' },
    display_title: 'Release v0.1.0 (dry run)' };
  validateSourceRun(record, commit, tag);
  for (const changed of [
    { head_sha: 'b'.repeat(40) }, { path: '.github/workflows/unreviewed.yml' },
    { display_title: 'Release unreviewed-branch (dry run)' }, { conclusion: 'failure' },
    { head_repository: { full_name: 'someone/fork' } },
  ]) assert.throws(() => validateSourceRun({ ...record, ...changed }, commit, tag), /tag-dispatched run/);
});

test('Windows launcher executable-mode variation never hides changed payloads', async () => {
  const { verifyLauncherAgreement } = await import('./release.mjs');
  const original = [{ name: 'package/bin/hpdev.cjs', mode: '-rwxr-xr-x', sha256: 'same' },
    { name: 'package/README.md', mode: '-rw-r--r--', sha256: 'readme' }];
  const windows = structuredClone(original);
  windows[0].mode = '-rw-r--r--';
  verifyLauncherAgreement(original, windows, 'win32');
  assert.throws(() => verifyLauncherAgreement(original, windows, 'linux'), /Conflicting launcher/);
  windows[0].sha256 = 'changed';
  assert.throws(() => verifyLauncherAgreement(original, windows, 'win32'), /Conflicting launcher/);
  windows[0].sha256 = 'same';
  windows[1].mode = '-rwxr-xr-x';
  assert.throws(() => verifyLauncherAgreement(original, windows, 'win32'), /Conflicting launcher/);
});
