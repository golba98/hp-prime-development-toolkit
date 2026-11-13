// Release validation and publication. Only the explicit `publish` command writes to npm.
import fs from 'node:fs/promises';
import { checkPaths } from '../check-docs.mjs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../..',
);
export const platforms = [
  ['x86_64-unknown-linux-gnu', 'linux', 'x64'],
  ['aarch64-unknown-linux-gnu', 'linux', 'arm64'],
  ['x86_64-pc-windows-msvc', 'win32', 'x64'],
  ['aarch64-apple-darwin', 'darwin', 'arm64'],
  ['x86_64-apple-darwin', 'darwin', 'x64'],
];
// The original recovery bundle is immutable and predates desktop packages.
export const legacyPlatforms = platforms.map(([target, os, cpu]) => [
  target.replace('-linux-gnu', '-linux-musl'),
  os,
  cpu,
]);
export const isMuslBundle = (manifest) =>
  manifest.native.some((entry) => entry.file.includes('-linux-musl.'));
const originalManifestSha256 =
  '00452ea100e849ac7f74f7841d10c7aba717aac77cf030d51c1b97822af64c17';
export const nativeArchiveName = (version, target, legacy = false) =>
  `hpdev-${version}-${target}.${target.includes('-windows-') && !legacy ? 'zip' : 'tar.gz'}`;
const python =
  process.env.PYTHON || (process.platform === 'win32' ? 'python' : 'python3');
const zipTool = fileURLToPath(new URL('./native_zip.py', import.meta.url));
const repository =
  'git+https://github.com/golba98/hp-prime-development-toolkit.git';
const registry = 'https://registry.npmjs.org';
const githubRepository = 'golba98/hp-prime-development-toolkit';
const digest = (data, algorithm = 'sha256', encoding = 'hex') =>
  createHash(algorithm).update(data).digest(encoding);
const integrity = (data) => `sha512-${digest(data, 'sha512', 'base64')}`;
const json = async (file) => JSON.parse(await fs.readFile(file, 'utf8'));
const run = (command, args, cwd = root) =>
  execFileSync(command, args, {
    cwd,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    shell: command === 'npm.cmd',
    maxBuffer: 16 * 1024 * 1024,
  });
function ensure(condition, message) {
  if (!condition) throw new Error(message);
}
function tar(file, member) {
  return tarBytes(file, member).toString('utf8');
}
function tarBytes(file, member) {
  return file.endsWith('.zip')
    ? execFileSync(python, [zipTool, 'read', file, member], {
        maxBuffer: 128 * 1024 * 1024,
      })
    : execFileSync('tar', ['-xOf', file, member], {
        maxBuffer: 128 * 1024 * 1024,
      });
}
function entries(file) {
  if (file.endsWith('.zip')) {
    const rows = JSON.parse(run(python, [zipTool, 'list', file]));
    const names = rows.map((row) => row.name);
    checkPaths(
      names
        .filter((name) => !name.endsWith('/'))
        .map((name) => name.split('/').slice(1).join('/')),
      undefined,
      { archive: true },
    );
    return names;
  }
  const names = run('tar', ['-tzf', file]).trim().split(/\r?\n/);
  ensure(
    names.every(
      (name) =>
        !/[\\\\:]/.test(name) &&
        name
          .replace(/\/$/, '')
          .split('/')
          .every((part) => part && part !== '.' && part !== '..'),
    ),
    `Unsafe archive paths: ${file}`,
  );
  ensure(
    new Set(names.map((name) => name.replace(/\/$/, ''))).size === names.length,
    `Duplicate archive members: ${file}`,
  );
  ensure(
    run('tar', ['-tvzf', file])
      .trim()
      .split(/\r?\n/)
      .every((line) => /^[d-]/.test(line)),
    `Archive contains links or special files: ${file}`,
  );
  checkPaths(
    names
      .filter((name) => !name.endsWith('/'))
      .map((name) => name.split('/').slice(1).join('/')),
    undefined,
    { archive: true },
  );
  return names;
}
export function validateMetadata(cargoText, npm) {
  const section = cargoText.match(
    /\[workspace\.package\]([^]*?)(?=\n\[|$)/,
  )?.[1];
  const cargo = section?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  ensure(
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(cargo),
    'A stable Cargo workspace version is required',
  );
  ensure(
    npm.name === 'hp-prime-devkit' && npm.version === cargo,
    'Cargo and npm versions must match',
  );
  const names = platforms.map(([, os, cpu]) => `hp-prime-devkit-${os}-${cpu}`);
  ensure(
    npm.optionalDependencies &&
      Object.keys(npm.optionalDependencies).length === names.length &&
      names.every((name) => npm.optionalDependencies[name] === cargo),
    'All five platform dependencies must match the release version exactly',
  );
  return npm;
}
export async function metadata(directory = root) {
  return validateMetadata(
    await fs.readFile(path.join(directory, 'Cargo.toml'), 'utf8'),
    await json(path.join(directory, 'npm/package.json')),
  );
}
export function resolveRef(
  ref,
  publish,
  directory = root,
  workflowRef = process.env.GITHUB_REF,
) {
  ensure(
    ref && !ref.startsWith('-') && !/[\r\n]/.test(ref),
    'Invalid release ref',
  );
  if (publish) {
    ensure(
      /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(ref),
      'Publish requires a stable vMAJOR.MINOR.PATCH tag',
    );
    ensure(
      workflowRef === `refs/tags/${ref}`,
      'Publish must be dispatched from the selected version tag',
    );
  }
  const sha = run(
    'git',
    [
      'rev-parse',
      '--verify',
      '--end-of-options',
      `${publish ? `refs/tags/${ref}` : ref}^{commit}`,
    ],
    directory,
  ).trim();
  if (publish)
    run('git', ['merge-base', '--is-ancestor', sha, 'origin/main'], directory);
  return sha;
}
export async function validateRef(directory = root) {
  const ref = process.env.RELEASE_REF || 'main';
  const publish = process.env.RELEASE_PUBLISH === 'true';
  const sha = resolveRef(ref, publish, directory);
  if (publish)
    ensure(
      process.env.GITHUB_SHA === sha,
      'Workflow provenance commit differs from the release tag',
    );
  // Read the selected commit, not whichever branch hosts the dispatched workflow.
  const npm = JSON.parse(
    run('git', ['show', `${sha}:npm/package.json`], directory),
  );
  validateMetadata(run('git', ['show', `${sha}:Cargo.toml`], directory), npm);
  const namedRef = run(
    'git',
    ['rev-parse', '--symbolic-full-name', ref],
    directory,
  ).trim();
  if (publish || namedRef.startsWith('refs/tags/'))
    ensure(
      namedRef === `refs/tags/v${npm.version}`,
      'Tag and package versions must match',
    );
  const matrix = platforms.map(([target, os, cpu], index) => ({
    target,
    os,
    cpu,
    runner: [
      'ubuntu-24.04',
      'ubuntu-24.04-arm',
      'windows-2022',
      'macos-15',
      'macos-15-intel',
    ][index],
  }));
  const result = {
    sha,
    version: npm.version,
    tag: `v${npm.version}`,
    matrix: JSON.stringify({ include: matrix }),
  };
  if (process.env.GITHUB_OUTPUT)
    await fs.appendFile(
      process.env.GITHUB_OUTPUT,
      Object.entries(result)
        .map(([key, value]) => `${key}=${value}\n`)
        .join(''),
    );
  console.log(JSON.stringify(result));
  return result;
}
export function inspectPackage(file, expected) {
  const names = entries(file);
  const pkg = JSON.parse(tar(file, 'package/package.json'));
  ensure(
    pkg.name === expected.name && pkg.version === expected.version,
    `Incorrect package identity: ${file}`,
  );
  ensure(
    pkg.repository?.url === repository,
    `Missing repository metadata: ${file}`,
  );
  for (const required of ['LICENSE', 'THIRD_PARTY_NOTICES.md'])
    ensure(
      names.includes(`package/${required}`),
      `Missing ${required}: ${file}`,
    );
  if (expected.os) {
    ensure(
      JSON.stringify(pkg.os) === JSON.stringify([expected.os]) &&
        JSON.stringify(pkg.cpu) === JSON.stringify([expected.cpu]),
      `Incorrect platform: ${file}`,
    );
    if (expected.os === 'linux' && !expected.legacyMusl)
      ensure(
        JSON.stringify(pkg.libc) === JSON.stringify(['glibc']),
        'Linux desktop packages require glibc metadata',
      );
    const executable = expected.os === 'win32' ? 'hpdev.exe' : 'hpdev';
    ensure(
      names.includes('package/licenses/inventory.json'),
      `Missing dependency notices: ${file}`,
    );
    const hash = digest(tarBytes(file, `package/bin/${executable}`));
    ensure(
      tar(file, 'package/SHA256SUMS') === `${hash}  bin/${executable}\n`,
      `Binary checksum mismatch: ${file}`,
    );
    return hash;
  }
  for (const required of [
    'bin/hpdev.cjs',
    'examples/SQUARE.ppl',
    'docs/packaging.md',
    'tests/snapshots/DRAW.png',
  ])
    ensure(
      names.includes(`package/${required}`),
      `Missing ${required}: ${file}`,
    );
  ensure(
    JSON.stringify(pkg.optionalDependencies) ===
      JSON.stringify(expected.optionalDependencies),
    'Launcher platform dependencies differ from the release metadata',
  );
}
function checksumList(manifest) {
  return [
    ...manifest.npm.map((entry) => ({
      file: entry.file,
      sha256: entry.sha256,
    })),
    ...manifest.native,
  ]
    .map((entry) => `${entry.sha256}  ${entry.file}\n`)
    .join('');
}
function launcherMembers(file) {
  const names = entries(file);
  const rows = run('tar', ['-tvzf', file]).trim().split(/\r?\n/);
  return names.map((name, index) => ({
    name,
    mode: rows[index].slice(0, 10),
    sha256: name.endsWith('/') ? '' : digest(tarBytes(file, name)),
  }));
}
export function verifyLauncherAgreement(canonical, candidate, os) {
  const expected = new Map(canonical.map((entry) => [entry.name, entry]));
  ensure(
    candidate.length === expected.size &&
      candidate.every((entry) => {
        const original = expected.get(entry.name);
        expected.delete(entry.name);
        if (!original || original.sha256 !== entry.sha256) return false;
        const windowsBinMode =
          os === 'win32' &&
          entry.name === 'package/bin/hpdev.cjs' &&
          original.mode === '-rwxr-xr-x' &&
          entry.mode === '-rw-r--r--';
        return original.mode === entry.mode || windowsBinMode;
      }),
    'Conflicting launcher payloads or file permissions',
  );
}
export function inspectNative(file, version, target, os, binaryHash) {
  const names = entries(file);
  const base = `hpdev-${version}-${target}`;
  const executable = os === 'win32' ? 'hpdev.exe' : 'hpdev';
  ensure(
    names.every((name) => name === base || name.startsWith(base + '/')),
    `Unexpected native archive root: ${file}`,
  );
  for (const required of [
    executable,
    'LICENSE',
    'THIRD_PARTY_NOTICES.md',
    'licenses/inventory.json',
    'README.md',
    'docs/packaging.md',
    'examples/SQUARE.ppl',
  ])
    ensure(
      names.includes(`${base}/${required}`),
      `Missing ${required}: ${file}`,
    );
  ensure(
    digest(tarBytes(file, `${base}/${executable}`)) === binaryHash,
    `Native and npm executables differ: ${file}`,
  );
  ensure(
    tar(file, `${base}/SHA256SUMS`) === `${binaryHash}  ${executable}\n`,
    `Native binary checksum mismatch: ${file}`,
  );
}
export async function prepare(source, output) {
  const pkg = await metadata();
  ensure(
    !process.env.RELEASE_VERSION || pkg.version === process.env.RELEASE_VERSION,
    'Unexpected release version',
  );
  const directories = await fs.readdir(source, { withFileTypes: true });
  const expectedDirectories = new Set(
    platforms.map(([target]) => `hpdev-${target}`),
  );
  ensure(
    directories.length === expectedDirectories.size &&
      directories.every(
        (entry) =>
          entry.isDirectory() && expectedDirectories.delete(entry.name),
      ),
    'Missing, duplicate or unexpected platform artifacts',
  );
  await fs.mkdir(output, { recursive: true });
  ensure(
    (await fs.readdir(output)).length === 0,
    'Release output directory must be empty',
  );
  const manifest = {
    version: pkg.version,
    commit: process.env.RELEASE_SHA || run('git', ['rev-parse', 'HEAD']).trim(),
    npm: [],
    native: [],
  };
  let launcherIntegrity, canonicalMembers;
  for (const [index, [target, os, cpu]] of platforms.entries()) {
    const directory = path.join(source, `hpdev-${target}`);
    const platform = {
      name: `hp-prime-devkit-${os}-${cpu}`,
      version: pkg.version,
      os,
      cpu,
    };
    const archive = nativeArchiveName(pkg.version, target);
    const expectedFiles = new Set([
      `${platform.name}-${pkg.version}.tgz`,
      `${pkg.name}-${pkg.version}.tgz`,
      archive,
      `${archive}.sha256`,
    ]);
    const files = await fs.readdir(directory, { withFileTypes: true });
    ensure(
      files.length === expectedFiles.size &&
        files.every(
          (entry) => entry.isFile() && expectedFiles.delete(entry.name),
        ),
      `Missing or unexpected files for ${target}`,
    );
    // Windows npm does not retain the Unix launcher executable bit. Compare all
    // payloads and allow only that known mode difference; publish Linux x64 bytes.
    const launcher = path.join(directory, `${pkg.name}-${pkg.version}.tgz`);
    inspectPackage(launcher, pkg);
    const currentIntegrity = integrity(await fs.readFile(launcher));
    const members = launcherMembers(launcher);
    if (index === 0) {
      launcherIntegrity = currentIntegrity;
      canonicalMembers = members;
    }
    verifyLauncherAgreement(canonicalMembers, members, os);
    ensure(
      os === 'win32' || currentIntegrity === launcherIntegrity,
      `Conflicting launcher artifacts from ${target}`,
    );
    if (currentIntegrity !== launcherIntegrity)
      console.log(
        'Validated Windows launcher mode variation; retaining the exact Linux x64 launcher tarball',
      );
    let binaryHash;
    for (const expected of index === 0 ? [platform, pkg] : [platform]) {
      const file = `${expected.name}-${pkg.version}.tgz`;
      const origin = path.join(directory, file);
      const hash = inspectPackage(origin, expected);
      if (expected.os) binaryHash = hash;
      const bytes = await fs.readFile(origin);
      await fs.copyFile(origin, path.join(output, file));
      manifest.npm.push({
        file,
        name: expected.name,
        version: pkg.version,
        integrity: integrity(bytes),
        sha256: digest(bytes),
        ...(expected.os ? { os, cpu } : {}),
      });
    }
    const origin = path.join(directory, archive);
    const hash = digest(await fs.readFile(origin));
    const checksum = `${hash}  ${archive}\n`;
    ensure(
      (await fs.readFile(`${origin}.sha256`, 'utf8')) === checksum,
      `Archive checksum mismatch: ${archive}`,
    );
    inspectNative(origin, pkg.version, target, os, binaryHash);
    await fs.copyFile(origin, path.join(output, archive));
    await fs.writeFile(path.join(output, `${archive}.sha256`), checksum);
    manifest.native.push({ file: archive, sha256: hash });
  }
  await fs.writeFile(path.join(output, 'SHA256SUMS'), checksumList(manifest));
  await fs.writeFile(
    path.join(output, 'release-manifest.json'),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  await validateBundle(output);
  console.log(
    `Validated ${manifest.npm.length} npm packages and ${manifest.native.length} native archives for ${pkg.version}`,
  );
  return manifest;
}
export async function validateBundle(directory) {
  const manifestBytes = await fs.readFile(
    path.join(directory, 'release-manifest.json'),
  );
  const manifest = JSON.parse(manifestBytes);
  // Only these exact, previously reviewed bytes may select the historical ABI.
  const legacyMusl = digest(manifestBytes) === originalManifestSha256;
  const bundlePlatforms = legacyMusl ? legacyPlatforms : platforms;
  const pkg = await metadata();
  const expectedCommit =
    process.env.RELEASE_SHA || run('git', ['rev-parse', 'HEAD']).trim();
  ensure(
    manifest.version === pkg.version &&
      /^[a-f0-9]{40}$/.test(manifest.commit) &&
      manifest.commit === expectedCommit,
    'Release manifest does not match the selected commit',
  );
  ensure(
    Array.isArray(manifest.npm) &&
      Array.isArray(manifest.native) &&
      manifest.npm.length === 6 &&
      manifest.native.length === 5,
    'Incomplete release manifest',
  );
  const expectedNames = new Set([
    pkg.name,
    ...Object.keys(pkg.optionalDependencies),
  ]);
  const binaryHashes = new Map();
  for (const entry of manifest.npm) {
    ensure(
      entry.version === pkg.version &&
        path.basename(entry.file) === entry.file &&
        entry.file === `${entry.name}-${pkg.version}.tgz` &&
        expectedNames.delete(entry.name),
      'Unexpected or duplicate npm artifact',
    );
    const platform = bundlePlatforms.find(
      ([, os, cpu]) => entry.name === `hp-prime-devkit-${os}-${cpu}`,
    );
    ensure(
      !platform || (entry.os === platform[1] && entry.cpu === platform[2]),
      'Incorrect manifest platform',
    );
    const file = path.join(directory, entry.file);
    const bytes = await fs.readFile(file);
    ensure(
      integrity(bytes) === entry.integrity && digest(bytes) === entry.sha256,
      `Artifact integrity mismatch: ${entry.file}`,
    );
    const binaryHash = inspectPackage(
      file,
      platform
        ? {
            name: entry.name,
            version: pkg.version,
            os: platform[1],
            cpu: platform[2],
            legacyMusl,
          }
        : pkg,
    );
    if (platform) binaryHashes.set(platform[0], binaryHash);
  }
  const expectedArchives = new Set(
    bundlePlatforms.map(([target]) =>
      nativeArchiveName(pkg.version, target, legacyMusl),
    ),
  );
  for (const entry of manifest.native) {
    ensure(
      expectedArchives.delete(entry.file),
      'Unexpected or duplicate native artifact',
    );
    const [target, os] = bundlePlatforms.find(
      ([target]) =>
        entry.file === nativeArchiveName(pkg.version, target, legacyMusl),
    );
    const file = path.join(directory, entry.file);
    ensure(
      digest(await fs.readFile(file)) === entry.sha256,
      `Artifact checksum mismatch: ${entry.file}`,
    );
    ensure(
      (await fs.readFile(`${file}.sha256`, 'utf8')) ===
        `${entry.sha256}  ${entry.file}\n`,
      'Archive checksum file differs',
    );
    inspectNative(file, pkg.version, target, os, binaryHashes.get(target));
  }
  const expectedFiles = new Set([
    'release-manifest.json',
    'SHA256SUMS',
    ...manifest.npm.map((entry) => entry.file),
    ...manifest.native.flatMap((entry) => [entry.file, `${entry.file}.sha256`]),
  ]);
  const files = await fs.readdir(directory, { withFileTypes: true });
  ensure(
    files.length === expectedFiles.size &&
      files.every(
        (entry) => entry.isFile() && expectedFiles.delete(entry.name),
      ),
    'Unexpected or duplicate release bundle files',
  );
  ensure(
    (await fs.readFile(path.join(directory, 'SHA256SUMS'), 'utf8')) ===
      checksumList(manifest),
    'Release SHA256SUMS differs',
  );
  return manifest;
}
export async function registryVersion(name, version) {
  const response = await fetch(
    `${registry}/${encodeURIComponent(name)}/${encodeURIComponent(version)}`,
    { signal: AbortSignal.timeout(30000) },
  );
  if (response.status === 404) return null;
  ensure(response.ok, `Registry lookup failed for ${name}: ${response.status}`);
  return response.json();
}
export async function ensurePublished(
  entry,
  lookup = registryVersion,
  wait = () => new Promise((resolve) => setTimeout(resolve, 5000)),
) {
  for (let attempt = 0; attempt < 12; attempt++) {
    const found = await lookup(entry.name, entry.version);
    if (found) {
      ensure(
        found.dist?.integrity === entry.integrity,
        `Published integrity differs for ${entry.name}@${entry.version}; do not overwrite or rebuild this version`,
      );
      return;
    }
    if (attempt < 11) await wait();
  }
  throw new Error(
    `Registry did not expose ${entry.name}@${entry.version}; rerun the publish job using the preserved artifacts`,
  );
}
// Read-only verification for releases whose exact tarballs already exist on npm.
export async function verifyPublishedBundle(
  directory,
  { lookup = registryVersion, wait } = {},
) {
  const manifest = await validateBundle(directory);
  for (const entry of manifest.npm) await ensurePublished(entry, lookup, wait);
  return manifest;
}
export async function publish(
  directory,
  {
    lookup = registryVersion,
    upload = (file) =>
      run('npm', [
        'publish',
        file,
        '--access',
        'public',
        '--tag',
        'latest',
        '--ignore-scripts',
        '--registry',
        registry,
      ]),
    wait,
  } = {},
) {
  ensure(
    process.env.RELEASE_PUBLISH === 'true',
    'Publication requires RELEASE_PUBLISH=true',
  );
  const manifest = await validateBundle(directory);
  // Order is intentional: never publish the launcher before all its dependencies exist.
  const ordered = [
    ...manifest.npm.filter((entry) => entry.name !== 'hp-prime-devkit'),
    manifest.npm.find((entry) => entry.name === 'hp-prime-devkit'),
  ];
  // Detect collisions across the entire release before uploading anything.
  for (const entry of ordered) {
    const found = await lookup(entry.name, entry.version);
    ensure(
      !found || found.dist?.integrity === entry.integrity,
      `Existing version differs: ${entry.name}@${entry.version}`,
    );
  }
  for (const entry of ordered) {
    const found = await lookup(entry.name, entry.version);
    if (found)
      ensure(
        found.dist?.integrity === entry.integrity,
        `Existing version differs: ${entry.name}@${entry.version}`,
      );
    else await upload(path.resolve(directory, entry.file));
    await ensurePublished(entry, lookup, wait);
    console.log(`Verified ${entry.name}@${entry.version}`);
  }
}
export async function githubApi(endpoint) {
  const response = await fetch(
    `https://api.github.com/repos/golba98/hp-prime-development-toolkit/${endpoint}`,
    {
      headers: {
        Authorization: `Bearer ${process.env.GH_TOKEN}`,
        Accept: 'application/vnd.github+json',
      },
      signal: AbortSignal.timeout(30000),
    },
  );
  if (response.status === 404) return null;
  ensure(response.ok, `GitHub API lookup failed: ${response.status}`);
  return response.json();
}
export function validateSourceRun(record, commit, tag) {
  // GitHub can populate `name` from run-name (for example "Release v0.1.0
  // (dry run)"). Identify the workflow by its path and execution provenance.
  ensure(
    record?.path === '.github/workflows/release.yml' &&
      record.event === 'workflow_dispatch' &&
      record.status === 'completed' &&
      record.conclusion === 'success' &&
      record.head_branch === tag &&
      record.head_sha === commit &&
      record.head_repository?.full_name === githubRepository &&
      [`Release ${tag} (dry run)`, `Release ${tag} (publish)`].includes(
        record.display_title,
      ),
    'Bundle must come from a successful tag-dispatched run of the selected source workflow',
  );
}
export async function verifySourceRun({
  id = process.env.BUNDLE_RUN_ID,
  commit = process.env.RELEASE_SHA,
  tag = process.env.RELEASE_TAG,
  api = githubApi,
} = {}) {
  ensure(/^[0-9]+$/.test(id), 'Invalid bundle workflow run ID');
  const record = await api(`actions/runs/${id}`);
  validateSourceRun(record, commit, tag);
  return record;
}
async function assetBytes(asset) {
  const response = await fetch(asset.url, {
    headers: {
      Authorization: `Bearer ${process.env.GH_TOKEN}`,
      Accept: 'application/octet-stream',
    },
    signal: AbortSignal.timeout(60000),
  });
  ensure(response.ok, `Cannot download release asset: ${asset.name}`);
  return Buffer.from(await response.arrayBuffer());
}
async function findRelease(tag, api) {
  const matches = [];
  // The by-tag endpoint is for published releases. REST listing includes drafts
  // visible to the authenticated repository token, including on retries.
  for (let page = 1; ; page++) {
    const releases = await api(`releases?per_page=100&page=${page}`);
    ensure(Array.isArray(releases), 'Cannot list GitHub releases');
    matches.push(...releases.filter((release) => release.tag_name === tag));
    if (releases.length < 100) break;
  }
  ensure(matches.length <= 1, 'Duplicate releases for the selected tag');
  return matches[0] || null;
}
export async function githubRelease(
  directory,
  finalize = false,
  {
    api = githubApi,
    download = assetBytes,
    command = run,
    lookup = registryVersion,
    tagCommit = (tag) =>
      run('git', ['rev-parse', '--verify', `refs/tags/${tag}^{commit}`]).trim(),
  } = {},
) {
  ensure(
    process.env.RELEASE_PUBLISH === 'true',
    'Release creation requires RELEASE_PUBLISH=true',
  );
  const manifest = await verifyPublishedBundle(directory, { lookup });
  const tag = `v${manifest.version}`;
  ensure(
    (await tagCommit(tag)) === manifest.commit,
    'Release tag moved or does not match the bundle',
  );
  let release = await findRelease(tag, api);
  if (!release) {
    ensure(
      !finalize,
      'Verify draft archive installation before making a release public',
    );
    command('gh', [
      'release',
      'create',
      tag,
      '--verify-tag',
      '--draft',
      '--generate-notes',
      '--title',
      `hpdev ${tag}`,
      '--repo',
      githubRepository,
    ]);
    release = await findRelease(tag, api);
  }
  ensure(
    release && release.tag_name === tag && !release.prerelease,
    'Unexpected GitHub release state',
  );
  const files = [
    ...manifest.native.flatMap((entry) => [entry.file, `${entry.file}.sha256`]),
    'release-manifest.json',
    'SHA256SUMS',
  ];
  const names = release.assets.map((asset) => asset.name);
  ensure(
    new Set(names).size === names.length &&
      names.every((name) => files.includes(name)),
    'Unexpected or duplicate release assets',
  );
  for (const file of files) {
    const existing = release.assets.find((asset) => asset.name === file);
    if (existing)
      ensure(
        digest(await download(existing)) ===
          digest(await fs.readFile(path.join(directory, file))),
        `Existing release asset differs: ${file}`,
      );
    else {
      ensure(
        !finalize && release.draft,
        'Refusing to finalize incomplete assets or change a public release',
      );
      command('gh', [
        'release',
        'upload',
        tag,
        path.resolve(directory, file),
        '--repo',
        githubRepository,
      ]);
    }
  }
  if (finalize && release.draft)
    command('gh', [
      'release',
      'edit',
      tag,
      '--draft=false',
      '--repo',
      githubRepository,
    ]);
  console.log(release.html_url);
  return release;
}

async function main() {
  const [command, source = 'artifacts', output = 'release'] =
    process.argv.slice(2);
  if (command === 'ref') await validateRef();
  else if (command === 'prepare')
    await prepare(path.resolve(source), path.resolve(output));
  else if (command === 'verify') await validateBundle(path.resolve(source));
  else if (command === 'verify-run') await verifySourceRun();
  else if (command === 'publish') await publish(path.resolve(source));
  else if (command === 'stage') await githubRelease(path.resolve(source));
  else if (command === 'complete')
    await githubRelease(path.resolve(source), true);
  else if (command === 'smoke' || command === 'native-smoke') {
    const { verifyNpm, verifyNative } = await import('./verify-install.mjs');
    if (command === 'smoke') await verifyNpm(path.resolve(source));
    else
      await verifyNative(path.resolve(source), {
        publicRelease: process.env.RELEASE_PUBLIC === 'true',
      });
  } else
    throw new Error(
      'Usage: node scripts/release/index.mjs ref|prepare [ARTIFACTS OUTPUT]|verify DIRECTORY|publish DIRECTORY|stage DIRECTORY|complete DIRECTORY|smoke DIRECTORY|native-smoke DIRECTORY',
    );
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  // Finish module evaluation before verify-install imports these shared exports.
  // Awaiting main here would deadlock the smoke commands' circular import.
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
