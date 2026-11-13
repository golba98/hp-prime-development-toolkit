import fs from 'node:fs/promises';
import path from 'node:path';

// Preserve installed example/fixture paths without shipping development test runners.
export async function copyProjectFiles(root, destination, copyOptions) {
  for (const file of [
    'README.md',
    'LICENSE',
    'THIRD_PARTY_NOTICES.md',
    'hpdev.toml',
  ]) {
    await fs.copyFile(path.join(root, file), path.join(destination, file));
  }
  for (const directory of [
    'examples',
    'docs',
    'tests/fixtures',
    'tests/snapshots',
  ]) {
    await fs.cp(
      path.join(root, directory),
      path.join(destination, directory),
      copyOptions,
    );
  }
}
