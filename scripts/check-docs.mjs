import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
export const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
);
export const policy = JSON.parse(
  fs.readFileSync(path.join(root, 'scripts/documentation-policy.json')),
);
const git = (args, cwd = root) =>
  execFileSync('git', args, {
    cwd,
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  });
const split = (s) => s.split('\0').filter(Boolean);
export function checkPaths(paths, config = policy, { archive = false } = {}) {
  for (const file of paths) {
    const name = file.replace(/\/$/, '');
    if (
      /^(?:agents(?:\.override)?|claude|gemini)\.md$/i.test(
        path.posix.basename(name),
      ) ||
      name.split('/').includes('.private') ||
      config.retiredPaths.includes(name)
    )
      throw Error(`Private or retired publication path: ${file}`);
    if (
      /\.(md|markdown|mdx)$/i.test(name) &&
      !(
        archive &&
        name.startsWith('licenses/') &&
        /^(?:\d+-)?(?:licen[cs]e|copying|notice|copyright|authors)([._-]|$)/i.test(
          path.posix.basename(name),
        )
      ) &&
      !Object.hasOwn(config.publicMarkdown, name)
    )
      throw Error(
        `Public Markdown requires reader/purpose review in scripts/documentation-policy.json: ${file}`,
      );
    if (!archive && /\.(zip|tgz|tar|tar\.gz|tar\.xz|7z)$/i.test(name))
      throw Error(
        `Tracked archives require explicit inspection support before approval: ${file}`,
      );
  }
}
export function checkGit({ cwd = root, mode = 'worktree', ref } = {}) {
  const staged = mode === 'staged';
  const config =
    mode === 'worktree' && !ref
      ? JSON.parse(
          fs.readFileSync(path.join(cwd, 'scripts/documentation-policy.json')),
        )
      : JSON.parse(
          git(
            ['show', `${staged ? '' : ref}:scripts/documentation-policy.json`],
            cwd,
          ),
        );
  const files = ref
    ? split(git(['ls-tree', '-rz', '--name-only', ref], cwd))
    : split(git(['ls-files', '-z'], cwd));
  // Even a missing working copy remains tracked until its deletion is staged.
  if (!ref && !staged)
    files.push(
      ...split(
        git(['ls-files', '-z', '--others', '--exclude-standard'], cwd),
      ).filter((file) => /\.(md|markdown|mdx)$/i.test(file)),
    );
  checkPaths(files, config);
}
if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    if (process.argv.includes('--pre-push')) {
      for (const row of fs
        .readFileSync(0, 'utf8')
        .trim()
        .split('\n')
        .filter(Boolean)) {
        const [, local, , remote] = row.split(/\s+/);
        if (/^0+$/.test(local)) continue;
        const commits = git([
          'rev-list',
          local,
          ...(/^0+$/.test(remote) ? ['--not', '--remotes'] : [`^${remote}`]),
        ])
          .trim()
          .split('\n')
          .filter(Boolean);
        for (const ref of new Set([local, ...commits])) checkGit({ ref });
      }
    } else
      checkGit(process.argv.includes('--staged') ? { mode: 'staged' } : {});
    console.log('Documentation publication check passed');
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
