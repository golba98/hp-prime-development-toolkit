import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { copyProjectFiles } from '../../scripts/release/project-files.mjs';

test('installed examples retain their paths without including development test suites', async (t) => {
  const temporary = await fs.mkdtemp(
    path.join(os.tmpdir(), 'hpdev-package-files-'),
  );
  t.after(() => fs.rm(temporary, { recursive: true, force: true }));
  const source = path.join(temporary, 'source');
  const destination = path.join(temporary, 'installed');
  const installed = [
    'README.md',
    'LICENSE',
    'THIRD_PARTY_NOTICES.md',
    'hpdev.toml',
    'examples/SQUARE.ppl',
    'docs/testing.md',
    'tests/fixtures/WAIT.ppl',
    'tests/snapshots/DRAW.png',
  ];
  const development = [
    'tests/browser/viewer.spec.mjs',
    'tests/desktop/run.py',
    'tests/packaging/release.test.mjs',
    'tests/packaging/fixtures/v0.1.0-release-manifest.json',
  ];
  for (const name of [...installed, ...development]) {
    const file = path.join(source, name);
    await fs.mkdir(path.dirname(file), { recursive: true });
    await fs.writeFile(file, name);
  }
  await fs.mkdir(destination);
  await copyProjectFiles(source, destination, { recursive: true });
  for (const name of installed) {
    assert.equal(await fs.readFile(path.join(destination, name), 'utf8'), name);
  }
  for (const name of development) {
    await assert.rejects(fs.access(path.join(destination, name)), {
      code: 'ENOENT',
    });
  }
});
