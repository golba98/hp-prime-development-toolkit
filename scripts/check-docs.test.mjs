import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { checkPaths, checkGit, policy } from './check-docs.mjs';
test('publication paths require explicit review and retain notices', () => {
  checkPaths(Object.keys(policy.publicMarkdown));
  for (const file of [
    '.private/evidence.json',
    'docs/.private/context.md',
    'docs/implementation-report.md',
    'docs/new-guide.md',
    'notes.zip',
  ])
    assert.throws(() => checkPaths([file]));
  checkPaths(
    ['licenses/dependency/LICENSE.md', 'docs/report.schema.json'],
    policy,
    { archive: true },
  );
  assert.throws(() =>
    checkPaths(['docs/implementation-report.md'], policy, { archive: true }),
  );
  assert.throws(() =>
    checkPaths(['licenses/notes.md'], policy, { archive: true }),
  );
});
test('staged checks use staged policy and catch force-added private bytes without touching the real index', (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'hpdev-docs-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const git = (args) => execFileSync('git', args, { cwd: dir, stdio: 'pipe' });
  git(['init', '-q']);
  fs.mkdirSync(path.join(dir, 'scripts'));
  const config = path.join(dir, 'scripts/documentation-policy.json');
  fs.writeFileSync(config, JSON.stringify(policy));
  fs.writeFileSync(path.join(dir, '.gitignore'), '/.private/\n');
  fs.writeFileSync(path.join(dir, 'README.md'), 'guide');
  git(['add', '.']);
  checkGit({ cwd: dir, mode: 'staged' });
  fs.mkdirSync(path.join(dir, '.private'));
  fs.writeFileSync(path.join(dir, '.private/notes.md'), 'private');
  git(['add', '.']);
  checkGit({ cwd: dir, mode: 'staged' });
  git(['add', '-f', '.private/notes.md']);
  assert.throws(() => checkGit({ cwd: dir, mode: 'staged' }), /Private/);
  git(['rm', '--cached', '.private/notes.md']);
  fs.writeFileSync(path.join(dir, 'new.md'), 'new');
  git(['add', 'new.md']);
  fs.writeFileSync(
    config,
    JSON.stringify({
      ...policy,
      publicMarkdown: {
        ...policy.publicMarkdown,
        'new.md': 'approved only in worktree',
      },
    }),
  );
  assert.throws(() => checkGit({ cwd: dir, mode: 'staged' }), /review/);
  checkGit({ cwd: dir });
  const unapprovedTree = git(['write-tree']).toString().trim();
  assert.throws(() => checkGit({ cwd: dir, ref: unapprovedTree }), /review/);
  git(['add', 'scripts/documentation-policy.json']);
  checkGit({ cwd: dir, ref: git(['write-tree']).toString().trim() });
});
