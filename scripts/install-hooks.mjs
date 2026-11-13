import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
const git = args => execFileSync('git', args, { encoding: 'utf8' }).trim();
let configured = '';
try { configured = git(['config', '--get', 'core.hooksPath']); } catch (error) { if (error.status !== 1) throw error; }
if (configured) throw Error('Existing core.hooksPath preserved; integrate check-docs.mjs manually.');
const hooks = path.resolve(git(['rev-parse', '--git-path', 'hooks']));
const commands = { 'pre-commit': '--staged', 'pre-push': '--pre-push' };
const bodies = Object.fromEntries(Object.entries(commands).map(([name, option]) => [name, `#!/bin/sh\ncd "$(git rev-parse --show-toplevel)" || exit 1\nexec node scripts/check-docs.mjs ${option}\n`]));
for (const [name, body] of Object.entries(bodies)) {
 const file = path.join(hooks, name);
 if (fs.existsSync(file) && fs.readFileSync(file, 'utf8') !== body) throw Error(`Existing hook preserved: ${file}`);
}
fs.mkdirSync(hooks, { recursive: true });
for (const [name, body] of Object.entries(bodies)) fs.writeFileSync(path.join(hooks, name), body, { mode: 0o755 });
console.log('Repository-local documentation hooks installed');
