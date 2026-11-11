# Native and npm distribution

No packages or public releases have been published. Publishing remains an
explicit maintainer action, separate from build and artifact assembly.

All Rust engine dependencies use the exact upstream Git revision and lockfile.
The npm package is only a launcher and exact-version optional dependencies:

| Runtime | Platform package |
| --- | --- |
| Linux x86_64 | `hp-prime-devkit-linux-x64` |
| Linux ARM64 | `hp-prime-devkit-linux-arm64` |
| Windows x86_64 | `hp-prime-devkit-win32-x64` |
| macOS ARM64 | `hp-prime-devkit-darwin-arm64` |
| macOS x86_64 | `hp-prime-devkit-darwin-x64` |

The launcher checks package version and executable SHA-256, forwards arguments
and interrupts, and preserves exit status. npm verifies package tarball
integrity during installation. No postinstall hook or separate downloader is
used. Native archives contain the executable, examples, notices, collected
dependency license files and a checksum manifest. Verify the archive's
`.sha256` before extracting a distributed artifact.

```sh
cargo build --locked --release --bin hpdev
node scripts/package.mjs --target x86_64-unknown-linux-gnu \
  --binary target/release/hpdev
node scripts/test-npm.mjs --binary target/release/hpdev
```

The install test packs the real root and platform packages, installs them
offline into a disposable global prefix, and runs the installed launcher from
outside either repository with Cargo absent from PATH. Nothing is published.

For portable Linux release builds use `x86_64-unknown-linux-musl` or
`aarch64-unknown-linux-musl` instead of GNU. GNU targets are supported by the
assembly script for local testing, but their binaries inherit host glibc
requirements. Platform packages share a name, so never mix GNU development
artifacts with the musl release set.

Before a future release, run the complete CI matrix, inspect collected notices
and archives, review dependency advisories, and verify installation on each
declared platform. Ordinary CI prepares packages and archives. The separate manual release
workflow described below publishes only when explicitly requested. Native users may copy
the extracted binary onto PATH without Node or Rust.

CI runner labels follow the [GitHub-hosted runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

## Manual release workflow

The **Manual release** workflow builds and validates a release without publishing
anything by default. It runs the same five-platform checks and browser tests as
ordinary CI. The repository does not yet have a public release.

In Actions, select **Manual release → Run workflow**:

| Input | Default | Behavior |
| --- | --- | --- |
| `ref` | `main` | Branch, commit, or tag to build; publication requires an existing stable `vMAJOR.MINOR.PATCH` tag |
| `publish` | `false` | Enable npm publication and GitHub release creation explicitly |
| `bundle_run_id` | empty | Reuse the exact `release-bundle` from an earlier successful manual dry run, instead of assembling a new bundle |

For publication, dispatch the workflow from `main`. The selected tag must point
to a commit in `main` and match the Cargo version, npm version, and all five
optional dependency versions. Change those versions together for each release;
this PR leaves the initial version at `0.1.0`. Prerelease tags are not supported.

A successful dry run produces a **release-bundle** artifact containing six npm
`.tgz` files, five native `.tar.gz` archives, their `.sha256` files, and
`release-manifest.json`. The manifest records the selected commit and npm
SHA-512 integrities. The canonical launcher package comes from Linux x64;
platform packages come from their respective native runners. All artifacts
include the relevant notices. The workflow validates archive contents,
platform metadata, binary checksums, and native/npm executable agreement.

Bundles are retained for 30 days. Download and retain a copy before publishing.
Dry runs do not create Git tags, npm packages, or GitHub releases.

## First npm publication

An npm account with permission to publish all six names is required. The names
must still be available when bootstrapping; a registry 404 does not reserve them.
The one-time initial publication uses your authenticated npm session. Do not
commit credentials or put a token in the workflow.

1. Merge the release preparation PR. Create and push `v0.1.0` at the intended
   commit on `main`, then run the workflow with `ref=v0.1.0` and `publish=false`.
2. Review the green platform/browser checks and the collected dependency notices.
   Download and extract its `release-bundle` into a directory outside the checkout.
   Keep the workflow run ID and the manifest's commit SHA.
3. Check out the exact tag. Run `npm login` interactively. Verify the bundle using
   the manifest's commit SHA (the commands below use a POSIX shell):

   ```sh
   export RELEASE_SHA="$(git rev-parse 'v0.1.0^{commit}')"
   node scripts/release.mjs verify /absolute/path/to/bundle
   ```

4. Publish the five `hp-prime-devkit-PLATFORM-0.1.0.tgz` files individually with
   `npm publish /absolute/path/to/FILE.tgz --access public --tag latest --ignore-scripts`.
   Complete npm's authentication prompts. Confirm each exact version is publicly
   available with `npm view PACKAGE@0.1.0 dist.integrity`; compare the returned
   integrity with the manifest. Then publish `hp-prime-devkit-0.1.0.tgz` the same way.
5. Configure a trusted publisher for **each of the six packages** on npmjs.com:
   owner `golba98`, repository `hp-prime-development-toolkit`, workflow filename
   `release.yml`, no environment restriction. Enable **Allow npm publish**.
   The workflow uses OIDC and automatic provenance for subsequent publications;
   it requires Node ≥22.14 and npm ≥11.5.1 and pins npm 11.21.0 on Node 24.
6. Run **Manual release** from `main` with `ref=v0.1.0`, `publish=true`, and
   `bundle_run_id` set to the reviewed dry-run ID. The workflow verifies existing
   npm versions against that exact bundle, tests public installation on all five
   platforms, and creates the GitHub release with native downloads and checksums.
7. Update the README's unreleased badge and installation wording only after
   publication and installation checks succeed.

New trusted publisher configurations must complete their first successful OIDC
publish within two days. The bootstrap version is already published, so the
finalization run above skips its uploads; configure trusted publishers just
before the next version's release, or recreate expired configurations then.
See [npm trusted publishing](https://docs.npmjs.com/trusted-publishers/) for
configuration, allowed actions, provenance, and expiry behavior.

## Later releases and recovery

For the next version, bump Cargo, the development package version/lockfile, npm,
and all five platform dependency versions together. Merge, tag that commit, and
run a dry run for the tag. Review and download its bundle, then dispatch a
publishing run with the same tag and `bundle_run_id` to reuse those exact files.

Publication checks all existing npm versions for integrity conflicts before
uploading anything. It publishes the five platform packages first, waits for
registry availability, then publishes the launcher. A mismatch stops the release;
never overwrite a published version or silently accept different bytes. Registry
availability checks are bounded, so npm scanning delays may require a retry.

Public installation checks run on every platform in a temporary prefix and
execute the included square example (`7` → `49`) outside the checkout. Only after
all checks pass does the workflow create a draft GitHub release, upload the five
archives, checksums, and manifest, and make the release public. Release assets
already present must match exactly. Publishing runs are serialized.

If a run fails after any npm upload, use **Re-run failed jobs** to retain the
successful preparation job and its exact bundle. Alternatively, reuse an earlier
successful dry-run ID with the same commit. Existing exact npm versions are
skipped; partially published versions are not rebuilt or unpublished. If a public
installation check fails, fix the issue and release a new version; npm cannot
roll back an immutable version. The GitHub release remains unpublished until
all platform installation checks pass.

To validate the orchestration locally without contacting npm:

```sh
node --test scripts/release.test.mjs
```
