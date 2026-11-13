// Assemble local npm packages. This script never downloads or publishes.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { collectLicenses } from './licenses.mjs';
import { copyProjectFiles } from './project-files.mjs';
import { checkPaths, policy } from '../check-docs.mjs';
import {
  inspectPackage,
  inspectNative,
  nativeArchiveName,
  platforms,
} from './index.mjs';
const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../..',
);
const python =
  process.env.PYTHON || (process.platform === 'win32' ? 'python' : 'python3');
// Exact-content documentation review before assembling public packages.
execFileSync(
  python,
  [
    path.join(root, 'scripts/check_documentation.py'),
    '--root',
    root,
    '--package-source',
  ],
  { stdio: ['ignore', 2, 2] },
);
const tracked = new Set(
  execFileSync('git', ['ls-files', '-z'], { cwd: root, encoding: 'utf8' })
    .split('\0')
    .filter(Boolean),
);
const approved = new Set([...tracked, ...Object.keys(policy.publicMarkdown)]);
const copyOptions = {
  recursive: true,
  filter: async (source) => {
    const relative = path.relative(root, source).split(path.sep).join('/');
    if (relative.split('/').includes('.private')) return false;
    const stat = await fs.lstat(source);
    if (stat.isSymbolicLink())
      throw new Error(`Publication input is a symlink: ${relative}`);
    if (stat.isDirectory()) return true;
    if (!approved.has(relative)) return false;
    checkPaths([relative]);
    return true;
  },
};

async function readableTree(directory) {
  await fs.chmod(directory, 0o755);
  for (const entry of await fs.readdir(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) await readableTree(file);
    else if (entry.isFile()) await fs.chmod(file, 0o644);
  }
}
const args = process.argv.slice(2);
const option = (key, fallback) => {
  const i = args.indexOf(key);
  return i < 0 ? fallback : args[i + 1];
};
const target = option('--target');
const binary = option('--binary');
const output = path.resolve(option('--output', path.join(root, 'dist')));
const targetPlatform = platforms.find(([rustTarget]) => rustTarget === target);
if (!targetPlatform || !binary)
  throw new Error(
    'Usage: node scripts/release/package.mjs --target RUST_TARGET --binary PATH [--output DIR]',
  );
const [, os, cpu] = targetPlatform;
execFileSync(
  python,
  [
    path.join(root, 'scripts/release/inspect_native.py'),
    '--target',
    target,
    '--binary',
    path.resolve(binary),
  ],
  { stdio: ['ignore', 2, 2] },
);
const metadata = JSON.parse(
  await fs.readFile(path.join(root, 'npm/package.json'), 'utf8'),
);
const cargoVersion = (
  await fs.readFile(path.join(root, 'Cargo.toml'), 'utf8')
).match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (
  metadata.version !== cargoVersion ||
  Object.values(metadata.optionalDependencies).some((v) => v !== cargoVersion)
)
  throw new Error(
    'Cargo, npm and platform dependency versions must match exactly.',
  );
const licenseDirectory = path.join(output, 'licenses');
await fs.rm(licenseDirectory, { recursive: true, force: true });
await collectLicenses(root, licenseDirectory);
const rootPackage = path.join(output, 'npm', metadata.name),
  name = `hp-prime-devkit-${os}-${cpu}`,
  platformPackage = path.join(output, 'npm', name);
await fs.rm(rootPackage, { recursive: true, force: true });
await fs.rm(platformPackage, { recursive: true, force: true });
await fs.mkdir(rootPackage, { recursive: true });
await fs.cp(path.join(root, 'npm'), rootPackage, copyOptions);
await copyProjectFiles(root, rootPackage, copyOptions);
await readableTree(rootPackage);
await fs.chmod(path.join(rootPackage, 'bin/hpdev.cjs'), 0o755);
await fs.mkdir(path.join(platformPackage, 'bin'), { recursive: true });
await fs.writeFile(
  path.join(platformPackage, 'package.json'),
  JSON.stringify(
    {
      name,
      version: metadata.version,
      description: `HP Prime PPL terminal toolkit: native executable for ${os} ${cpu}`,
      license: 'MIT',
      repository: metadata.repository,
      homepage: metadata.homepage,
      bugs: metadata.bugs,
      keywords: metadata.keywords,
      os: [os],
      cpu: [cpu],
      ...(os === 'linux' ? { libc: ['glibc'] } : {}),
      files: [
        'bin',
        'licenses',
        'LICENSE',
        'THIRD_PARTY_NOTICES.md',
        'SHA256SUMS',
      ],
    },
    null,
    2,
  ) + '\n',
);
const executable = os === 'win32' ? 'hpdev.exe' : 'hpdev';
const destination = path.join(platformPackage, 'bin', executable);
await fs.copyFile(path.resolve(binary), destination);
if (os !== 'win32') await fs.chmod(destination, 0o755);
const digest = createHash('sha256')
  .update(await fs.readFile(destination))
  .digest('hex');
await fs.writeFile(
  path.join(platformPackage, 'SHA256SUMS'),
  `${digest}  bin/${executable}\n`,
);
for (const file of ['LICENSE', 'THIRD_PARTY_NOTICES.md'])
  await fs.copyFile(path.join(root, file), path.join(platformPackage, file));
await fs.cp(licenseDirectory, path.join(platformPackage, 'licenses'), {
  recursive: true,
});
await readableTree(platformPackage);
if (os !== 'win32') await fs.chmod(destination, 0o755);
const nativeDirectory = path.join(
  output,
  `hpdev-${metadata.version}-${target}`,
);
await fs.rm(nativeDirectory, { recursive: true, force: true });
await fs.mkdir(nativeDirectory, { recursive: true });
await fs.copyFile(destination, path.join(nativeDirectory, executable));
if (os !== 'win32')
  await fs.chmod(path.join(nativeDirectory, executable), 0o755);
await copyProjectFiles(root, nativeDirectory, copyOptions);
await fs.cp(licenseDirectory, path.join(nativeDirectory, 'licenses'), {
  recursive: true,
});
await fs.writeFile(
  path.join(nativeDirectory, 'SHA256SUMS'),
  `${digest}  ${executable}\n`,
);
await readableTree(nativeDirectory);
if (os !== 'win32')
  await fs.chmod(path.join(nativeDirectory, executable), 0o755);
const archive = path.join(output, nativeArchiveName(metadata.version, target));
if (os === 'win32')
  execFileSync(process.env.PYTHON || 'python', [
    path.join(root, 'scripts/release/native_zip.py'),
    'create',
    archive,
    nativeDirectory,
  ]);
else
  execFileSync('tar', [
    '-czf',
    archive,
    '-C',
    path.dirname(nativeDirectory),
    path.basename(nativeDirectory),
  ]);
const archiveDigest = createHash('sha256')
  .update(await fs.readFile(archive))
  .digest('hex');
await fs.writeFile(
  `${archive}.sha256`,
  `${archiveDigest}  ${path.basename(archive)}\n`,
);
// Pack the exact publishable artifacts now, so retries never rebuild a version.
const npmTarballs = [platformPackage, rootPackage].map((directory) => {
  const packed = JSON.parse(
    execFileSync(
      process.platform === 'win32' ? 'npm.cmd' : 'npm',
      ['pack', '--json', '--ignore-scripts', '--pack-destination', output],
      {
        cwd: directory,
        encoding: 'utf8',
        shell: process.platform === 'win32',
        maxBuffer: 4 * 1024 * 1024,
      },
    ),
  );
  return path.join(output, packed[0].filename);
});
inspectPackage(npmTarballs[0], { name, version: metadata.version, os, cpu });
inspectPackage(npmTarballs[1], metadata);
inspectNative(archive, metadata.version, target, os, digest);
console.log(
  JSON.stringify({
    version: metadata.version,
    target,
    rootPackage,
    platformPackage,
    nativeArchive: archive,
    npmTarballs,
    sha256: digest,
  }),
);
