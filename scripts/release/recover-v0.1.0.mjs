// One-time, read-only npm recovery. These pins describe the original successful
// dry run, not a new build or a caller-selected artifact. Remove after recovery.
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import {
  githubApi,
  verifySourceRun,
  verifyPublishedBundle,
  legacyPlatforms as platforms,
} from './index.mjs';

export const recovery = Object.freeze({
  repository: 'golba98/hp-prime-development-toolkit',
  version: '0.1.0',
  tag: 'v0.1.0',
  commit: '435889a144b9cb011253eca51dc3c3916508e86c',
  tagObject: 'ab4ac14fe58f8825f7f97793ceae1c23d25b45c0',
  runId: '43124712655',
  workflowId: 415260017,
  runAttempt: 1,
  artifactId: 13902762171,
  artifactDigest:
    'sha256:f6d720f123433a450f1e3d53fd74fc1b78207d663cb8329a2bf9f39edfb9bfd1',
  manifestSha256:
    '00452ea100e849ac7f74f7841d10c7aba717aac77cf030d51c1b97822af64c17',
});
function ensure(condition, message) {
  if (!condition) throw new Error(message);
}

export function validateRecoveryContext(env) {
  ensure(
    env.GITHUB_REPOSITORY === recovery.repository &&
      env.GITHUB_EVENT_NAME === 'workflow_dispatch' &&
      env.GITHUB_REF === 'refs/heads/main' &&
      env.GITHUB_WORKFLOW_REF ===
        `${recovery.repository}/.github/workflows/recover-v0.1.0.yml@refs/heads/main` &&
      /^[a-f0-9]{40}$/.test(env.GITHUB_SHA) &&
      env.GITHUB_WORKFLOW_SHA === env.GITHUB_SHA,
    'Recovery must execute the reviewed recovery workflow from main in the original repository',
  );
  ensure(
    env.RELEASE_SHA === recovery.commit,
    'Recovery source commit must remain pinned',
  );
}

export async function verifyRecoverySource({
  api = githubApi,
  env = process.env,
} = {}) {
  validateRecoveryContext(env);
  const record = await verifySourceRun({
    id: recovery.runId,
    commit: recovery.commit,
    tag: recovery.tag,
    api,
  });
  ensure(
    String(record.id) === recovery.runId &&
      record.workflow_id === recovery.workflowId &&
      record.run_attempt === recovery.runAttempt &&
      record.display_title === `Release ${recovery.tag} (dry run)`,
    'Recovery requires the original successful dry-run attempt',
  );
  const artifact = await api(`actions/artifacts/${recovery.artifactId}`);
  ensure(
    artifact?.id === recovery.artifactId &&
      artifact.name === 'release-bundle' &&
      artifact.expired === false &&
      artifact.digest === recovery.artifactDigest &&
      String(artifact.workflow_run?.id) === recovery.runId &&
      artifact.workflow_run?.head_sha === recovery.commit &&
      artifact.workflow_run?.head_branch === recovery.tag,
    'Recovery requires the original unexpired release-bundle artifact and digest',
  );
  // Also pin the annotated tag object, so even a replacement annotation fails.
  const tag = await api(`git/ref/tags/${recovery.tag}`);
  ensure(
    tag?.object?.type === 'tag' && tag.object.sha === recovery.tagObject,
    'Original release tag changed',
  );
  const annotation = await api(`git/tags/${recovery.tagObject}`);
  ensure(
    annotation?.tag === recovery.tag &&
      annotation.object?.type === 'commit' &&
      annotation.object.sha === recovery.commit,
    'Original release tag does not resolve to the pinned commit',
  );
}

export function verifyRecoveryManifest(bytes) {
  // Independently bind the extracted manifest to the original reviewed bytes,
  // before validating its file hashes or executing any downloaded binary.
  ensure(
    createHash('sha256').update(bytes).digest('hex') ===
      recovery.manifestSha256,
    'Recovery manifest differs from the original dry-run bundle',
  );
}

export async function verifyRecoveryBundle(directory, options) {
  verifyRecoveryManifest(
    await fs.readFile(path.join(directory, 'release-manifest.json')),
  );
  // Validates every tarball, native binary, checksum, package identity and file
  // set, then requires matching dist.integrity for all six existing npm versions.
  return verifyPublishedBundle(directory, options);
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const [command, directory = 'release'] = process.argv.slice(2);
  ensure(
    ['source', 'verify'].includes(command),
    'Usage: node scripts/release/recover-v0.1.0.mjs source|verify [DIRECTORY]',
  );
  await verifyRecoverySource();
  if (command === 'verify') await verifyRecoveryBundle(path.resolve(directory));
  else {
    const runners = [
      'ubuntu-24.04',
      'ubuntu-24.04-arm',
      'windows-latest',
      'macos-15',
      'macos-15-intel',
    ];
    const matrix = {
      include: platforms.map(([target], index) => ({
        target,
        runner: runners[index],
      })),
    };
    if (process.env.GITHUB_OUTPUT)
      await fs.appendFile(
        process.env.GITHUB_OUTPUT,
        `matrix=${JSON.stringify(matrix)}\n`,
      );
  }
}
