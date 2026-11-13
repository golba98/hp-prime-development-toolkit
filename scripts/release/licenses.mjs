// Collect shipped dependency notices from the exact locked Cargo sources.
import fs from 'node:fs/promises';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
export async function collectLicenses(root, output) {
  const metadata = JSON.parse(
    execFileSync('cargo', ['metadata', '--locked', '--format-version=1'], {
      cwd: root,
      encoding: 'utf8',
      maxBuffer: 32 * 1024 * 1024,
    }),
  );
  await fs.mkdir(output, { recursive: true });
  const inventory = [];
  const desktopNotices = JSON.parse(
    await fs.readFile(
      path.join(root, 'docs/licenses/desktop-notices.json'),
      'utf8',
    ),
  );
  for (const pkg of metadata.packages) {
    if (pkg.name.startsWith('hpdev-')) continue;
    const directory = path.dirname(pkg.manifest_path);
    let candidates = [];
    const scan = async (dir) => {
      for (const entry of await fs.readdir(dir, { withFileTypes: true })) {
        if (
          entry.isFile() &&
          /^(licen[cs]e|copying|notice|copyright|authors)([._-]|$)/i.test(
            entry.name,
          )
        )
          candidates.push(path.join(dir, entry.name));
        if (entry.isDirectory() && /^licen[cs]es$/i.test(entry.name)) {
          for (const file of await fs.readdir(path.join(dir, entry.name))) {
            const p = path.join(dir, entry.name, file);
            if ((await fs.stat(p)).isFile()) candidates.push(p);
          }
        }
      }
    };
    await scan(directory);
    if (pkg.license_file)
      candidates.push(path.resolve(directory, pkg.license_file));
    if (!candidates.length && pkg.source?.startsWith('git+')) {
      await scan(path.resolve(directory, '../..'));
    }
    const desktopNotice = desktopNotices[pkg.name + '@' + pkg.version];
    if (!candidates.length && desktopNotice) {
      candidates.push(
        path.join(root, 'docs/licenses', desktopNotice),
        path.join(root, 'docs/licenses/MIT-terms.txt'),
      );
    }
    if (
      !candidates.length &&
      ['block2', 'dispatch2', 'objc2', 'objc2-encode'].includes(pkg.name)
    ) {
      candidates.push(
        path.join(root, 'docs/licenses', `${pkg.name}-${pkg.version}.txt`),
        path.join(root, 'docs/licenses/MIT-terms.txt'),
      );
    }
    if (
      !candidates.length &&
      pkg.name.startsWith('winapi-') &&
      pkg.version === '0.4.0'
    ) {
      const parent = metadata.packages.find(
        (p) => p.name === 'winapi' && p.version === '0.3.9',
      );
      if (parent)
        candidates.push(
          path.join(path.dirname(parent.manifest_path), 'LICENSE-MIT'),
          path.join(path.dirname(parent.manifest_path), 'LICENSE-APACHE'),
        );
    }
    candidates = [...new Set(candidates)];
    if (!candidates.length)
      throw new Error(
        `No license notice found for ${pkg.name}@${pkg.version}; review before distributing.`,
      );
    const destination = path.join(output, `${pkg.name}-${pkg.version}`);
    await fs.mkdir(destination, { recursive: true });
    const files = [];
    for (const [i, file] of candidates.entries()) {
      const name = `${i}-${path.basename(file)}`;
      await fs.copyFile(file, path.join(destination, name));
      files.push(name);
    }
    inventory.push({
      name: pkg.name,
      version: pkg.version,
      license: pkg.license,
      authors: pkg.authors,
      source: pkg.source,
      files,
    });
  }
  const tracked = new Set(
    execFileSync('git', ['ls-files', '-z', '--', 'docs/licenses'], {
      cwd: root,
      encoding: 'utf8',
    })
      .split('\0')
      .filter(Boolean),
  );
  await fs.cp(
    path.join(root, 'docs/licenses'),
    path.join(output, 'upstream-font'),
    {
      recursive: true,
      filter: async (source) => {
        if (source.split(path.sep).includes('.private')) return false;
        const stat = await fs.lstat(source);
        if (stat.isSymbolicLink())
          throw new Error('License input must not be a symlink');
        return (
          stat.isDirectory() ||
          tracked.has(path.relative(root, source).split(path.sep).join('/'))
        );
      },
    },
  );
  await fs.writeFile(
    path.join(output, 'inventory.json'),
    JSON.stringify(
      {
        note: 'Locked dependency inventory, including optional, build and test dependencies; not all entries are linked into each binary.',
        packages: inventory,
      },
      null,
      2,
    ) + '\n',
  );
  return inventory.length;
}
