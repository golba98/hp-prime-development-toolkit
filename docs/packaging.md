# Packaging and the first public release

The toolkit is **unpublished**. Building, packing and running a dry run do not
authorize publication. Do not create the release tag, bootstrap npm packages or
dispatch a publishing run until the maintainer explicitly approves that action.
The README's unpublished notice remains until public installation checks succeed.

## Distribution layout

The Rust workspace, npm launcher and all five optional dependency versions are
currently **0.1.0**. A publishing tag must be exactly `v0.1.0` and point to the
selected commit on `main`. Future versions change these values together.

| Runner | Rust target | npm platform package |
| --- | --- | --- |
| Ubuntu 24.04 x64 | `x86_64-unknown-linux-musl` | `hp-prime-devkit-linux-x64` |
| Ubuntu 24.04 ARM64 | `aarch64-unknown-linux-musl` | `hp-prime-devkit-linux-arm64` |
| Windows x64 | `x86_64-pc-windows-msvc` | `hp-prime-devkit-win32-x64` |
| macOS 15 Apple Silicon | `aarch64-apple-darwin` | `hp-prime-devkit-darwin-arm64` |
| macOS 15 Intel | `x86_64-apple-darwin` | `hp-prime-devkit-darwin-x64` |

The sixth package, **`hp-prime-devkit`**, supplies the `hpdev` command. Its
exact-version optional dependencies select a native package. The launcher verifies
the package version and executable SHA-256, forwards arguments and interrupts,
and preserves exit status. npm verifies tarball integrity during installation.
There is no postinstall hook, separate downloader or Rust build during npm install.

Standalone `.tar.gz` downloads include the executable, examples, configuration,
documentation, notices, collected dependency licenses and a binary checksum
manifest. They run without Rust or Node.js. GNU Linux builds remain useful for
local development, but releases use musl; never mix GNU and musl artifacts.

## Install a standalone download after publication

These commands are for the eventual public release; the download URLs do not
exist before publication. Choose the target in the distribution table.

On Linux x64 (no npm, Node or Rust needed):

```sh
curl -fLO https://github.com/golba98/hp-prime-development-toolkit/releases/download/v0.1.0/hpdev-0.1.0-x86_64-unknown-linux-musl.tar.gz
curl -fLO https://github.com/golba98/hp-prime-development-toolkit/releases/download/v0.1.0/hpdev-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256
sha256sum -c hpdev-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256
tar -xzf hpdev-0.1.0-x86_64-unknown-linux-musl.tar.gz
./hpdev-0.1.0-x86_64-unknown-linux-musl/hpdev --version
./hpdev-0.1.0-x86_64-unknown-linux-musl/hpdev --help
```

For Linux ARM64 substitute its target. On macOS choose the Intel or Apple Silicon
target and verify with `shasum -a 256 -c ARCHIVE.tar.gz.sha256` before extraction.
Copy the extracted `hpdev` onto a directory in PATH if desired.

On Windows, download the MSVC archive and its sidecar with a browser or
`Invoke-WebRequest`. In PowerShell, verify before extracting:

```powershell
$archive = 'hpdev-0.1.0-x86_64-pc-windows-msvc.tar.gz'
$expected = (Get-Content "$archive.sha256").Split(' ')[0]
if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Archive checksum mismatch' }
tar -xzf $archive
& .\hpdev-0.1.0-x86_64-pc-windows-msvc\hpdev.exe --version
& .\hpdev-0.1.0-x86_64-pc-windows-msvc\hpdev.exe --help
```

The sidecar checks the individual download. The aggregate checksum file also
covers npm tarballs retained in the workflow bundle; those tarballs are
distributed through npm rather than attached as standalone GitHub downloads.

## Local checks and assembly

Run the existing checks before requesting review:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets --no-default-features -- -D warnings
node --test scripts/release.test.mjs
npm ci --ignore-scripts
npx playwright install chromium
npm run test:browser
```

A local Linux x64 release-artifact check is:

```sh
rustup target add x86_64-unknown-linux-musl
cargo test --locked --workspace --target x86_64-unknown-linux-musl
cargo build --locked --release --target x86_64-unknown-linux-musl --bin hpdev
node scripts/test-npm.mjs --target x86_64-unknown-linux-musl --binary target/x86_64-unknown-linux-musl/release/hpdev
node scripts/package.mjs --target x86_64-unknown-linux-musl --binary target/x86_64-unknown-linux-musl/release/hpdev
node scripts/verify-install.mjs local dist x86_64-unknown-linux-musl
```

Use the corresponding target and executable on other systems. The offline npm
test packs the real packages, installs into a temporary global prefix and checks
version, help, the installed command shim, SQUARE execution and checksum rejection.
The standalone check extracts the archive outside the checkout and executes
version, help and SQUARE with Node and Rust absent from PATH.

License collection reads the exact Cargo Git revisions and lockfile. It fails
when a required notice cannot be found. Review the resulting inventory and
third-party notices before distribution. Dependency pins remain unchanged.

## What the workflows enforce

Ordinary CI and the manual release reuse the same five-platform build matrix.
Each runner executes Rust formatting, release-helper regression tests, target
Clippy, target Rust tests, no-default-features Clippy, a release build, the offline
npm installation and standalone archive checks. Browser integration runs
separately. Any failed target prevents distribution validation and publication;
matrix jobs use `fail-fast: false` and report each platform's result.

The manual workflow follows these gates:

1. **Validate:** resolve the requested commit; check the stable Cargo workspace
   version, launcher and exactly five dependency versions; enforce matching tags.
   Publication is dispatched from the selected version tag, which must be on
   `main`. The workflow SHA must match it so npm provenance names the exact
   artifact source.
2. **Check and prepare:** all CI gates must pass. Validate the complete artifact
   set and collect one source-bound bundle.
3. **Publish npm:** after protected environment approval, check all six registry
   versions for conflicts, then publish/verify all five platforms before the
   launcher. Only this job receives `id-token: write`.
4. **Test registry installation:** each of five runners installs the exact version
   from the actual npm registry into a fresh global prefix, verifies native
   dependency selection against the reviewed binary, and executes `--version`,
   `--help` and included SQUARE (`7 → 49`).
5. **Stage GitHub downloads:** create a draft for the existing tag and attach the
   five archives, their five checksum files, `SHA256SUMS` and the manifest.
6. **Test draft downloads:** all five runners download their actual GitHub draft
   assets, verify hashes and execute the standalone command without Rust/Node on PATH.
7. **Make public:** after all draft checks and environment approval, verify the
   existing assets again and make the release public.
8. **Verify publicly:** all five runners repeat the npm checks and download the
   public archives without GitHub authentication. A failure stays visible and
   blocks declaring the release complete; already public artifacts cannot be undone.

Every platform failure is reported in its job and step summary. Public verification
is deliberately absent during dry runs; it must not be described as passed before
publication. No workflow creates a Git tag.

Actions are pinned to commits. Release automation uses Node 24 and npm 11.21.0;
publishing has no long-lived npm token and package-manager caching is disabled.
The supported OIDC requirements and automatic provenance behavior are described in
[npm's trusted publishing guide](https://docs.npmjs.com/trusted-publishers/).
The [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
lists the native hosted runners.

## Manual dry run: no public side effects

After merging the workflow, select **Actions → Manual release → Run workflow**:

| Input | Default | Meaning |
| --- | --- | --- |
| `ref` | `main` | Branch/commit for a dry run, existing stable version tag for publication |
| `publish` | `false` | Explicitly enter publication gates |
| `bundle_run_id` | empty | Reuse the exact bundle from an earlier successful Manual release run |

For example:

```sh
gh workflow run release.yml --ref main -f ref=main -f publish=false
```

A dry run requires neither npm publisher configuration nor a release tag.
It builds and executes all target artifacts, then uploads a **release-bundle**
Actions artifact retained for **30 days**. It creates no npm packages or GitHub release.

The bundle contains six npm tarballs, five native archives, five archive checksum
sidecars, an aggregate `SHA256SUMS` covering the eleven tarballs, and
`release-manifest.json` with commit, version, SHA-256 and npm SHA-512 integrity.

Only the five expected platform directories and four expected files per directory
are accepted as input. All runner launcher payloads and file modes must agree, with one explicit
exception: Windows npm records the launcher executable as mode 0644 instead of
0755. That sole mode difference is logged and accepted only for the Windows
copy; file contents must still match. Linux x64 supplies the exact retained
tarball, and other runners must match it byte-for-byte. LF checkout rules keep
source bytes portable. Extra files, missing platforms, duplicate archive members, links,
conflicting launchers, wrong identities or mismatched native/npm binaries fail.
Reused bundles are fully revalidated against the selected commit.
For publication reuse, the source run must also have executed the reviewed
release workflow at that exact commit with the version tag as its input.
Dispatch tagged bundle dry runs from the tag itself; an arbitrary branch run
cannot qualify by putting a trusted commit into its manifest.

## Maintainer setup before first publication

All of this is external setup; the workflow does not create accounts or configure
npm package ownership, GitHub environments or branch/tag rules.

- Use the intended npm maintainer account. Enable **2FA for authentication and
  publishing**, retain recovery codes securely, and agree which maintainers will
  own all six unscoped names. Check name availability; a 404 does not reserve a name.
  If another account owns one, obtain legitimate ownership or choose new names and
  update every binding before proceeding.
- Create the GitHub environment **`npm-release`**, require maintainer review,
  prevent self-review where supported, and restrict deployments to version tags matching `v*` (no branches).
  Both npm publication and final GitHub visibility use this environment.
- Protect `main` and version tags against unauthorized changes. Permit GitHub
  Actions to create pull requests if the repository's contribution process needs it;
  release jobs themselves only require the permissions declared in the workflow.
- Ensure Actions and artifact storage are available, with access to all five hosted
  runners. Limit who can dispatch publishing workflows. Do not add `NPM_TOKEN`
  or `NODE_AUTH_TOKEN` publishing secrets.

### Bootstrap all six npm packages

npm trusted publishing cannot be configured until a package exists. The first
publication therefore uses a maintainer's interactive npm session, after explicit
approval. Do not publish placeholders or rebuild artifacts between bootstrap and
release finalization.

1. Merge this release infrastructure. Obtain explicit approval to create
   `v0.1.0` at the chosen reviewed `main` commit. The maintainer creates and
   pushes that tag; this work does not do so.
2. Run **Manual release** from tag `v0.1.0` with `ref=v0.1.0`, `publish=false`. Review every
   platform/browser result, notices and artifact contents. Download and retain
   its exact `release-bundle`, run ID and manifest commit outside the checkout.
3. Check out the same tag. Set `RELEASE_SHA` to the manifest commit and verify:

   ```sh
   export RELEASE_SHA="$(git rev-parse 'v0.1.0^{commit}')"
   node scripts/release.mjs verify /absolute/path/to/bundle
   ```

4. After explicit approval for npm bootstrap, use `npm login` interactively and
   `npm whoami` to confirm the intended account. Publish the five platform
   tarballs individually in this order, completing the 2FA prompts:
   Linux x64, Linux ARM64, Windows x64, macOS Apple Silicon, macOS Intel.
   Then publish the launcher tarball. For each reviewed file use:

   ```sh
   npm publish /absolute/path/to/FILE.tgz --access public --tag latest --ignore-scripts
   npm view PACKAGE@0.1.0 dist.integrity --registry https://registry.npmjs.org
   npm owner ls PACKAGE
   ```

   Compare each integrity with the manifest. A different existing version is a
   hard stop. Confirm ownership of **all six** names; add agreed co-maintainers
   deliberately using npm's ownership controls and require their 2FA too.
   Interactive bootstrap does not claim OIDC-generated provenance.
5. For each package's npm **Settings → Trusted publishing**, configure GitHub:
   user **golba98**, repository **hp-prime-development-toolkit**, workflow filename
   **release.yml**, environment **npm-release**, and **Allow npm publish**.
   Choose **Require two-factor authentication and disallow tokens** under
   publishing access. Normal automated releases use only OIDC.
6. A new trusted publisher must complete its first successful OIDC publication
   within **two days**. An exact-version skip during bootstrap finalization does
   not validate that configuration. Configure or recreate it just before the next
   new version if necessary; expiry is not evidence of a failed v0.1.0 bootstrap.
7. Obtain approval to finalize the public GitHub release. Dispatch the workflow
   from tag `v0.1.0`
   with `ref=v0.1.0`, `publish=true`, and `bundle_run_id=REVIEWED_RUN_ID`.
   Approve the environment gates. Existing exact npm uploads are verified and
   skipped; registry, draft archive and public download tests still run.
   The workflow selector and input must both name the same tag. For example,
   after explicit approval:

   ```sh
   gh workflow run release.yml --ref v0.1.0 -f ref=v0.1.0 -f publish=true -f bundle_run_id=REVIEWED_RUN_ID
   ```

8. Only after all five final public checks pass may a follow-up documentation
   change remove the README's unpublished notice.

## Later releases, retries and rollback limits

Bump the workspace version, development package/lockfile, launcher and all five
optional dependencies together. Review/merge, obtain tag approval, create the tag,
dry-run it, then reuse that reviewed bundle in the explicitly approved publish run.

Publishing runs are serialized. All six existing registry versions are checked
before any upload. Exact bytes are skipped; differing SHA-512 integrity aborts.
Every platform must become visible in the registry before the launcher is uploaded.

If a publishing job fails, use **Re-run failed jobs** so successful preparation and
its bundle are retained. Alternatively dispatch a new run with the same tag and a
successful earlier `bundle_run_id`. A reused bundle must have the exact commit
and full expected contents. Download a backup before the 30-day artifact expiry.
Do not rerun all jobs or rebuild a partially published version.

Existing GitHub assets are also byte-checked. Missing assets can be added only to
a draft. Conflicting or duplicate assets stop the operation; existing assets are
never overwritten, and public releases are never silently altered. Finalization
checks the tag still matches the bundle.

npm versions are immutable. A partially published release cannot be rolled back
atomically, and changing `latest` does not erase an installed version. If a public
installation fails, stop announcing the release, investigate, and prepare a new
patch version. Deprecation, dist-tag changes, unpublishing, release deletion or tag
changes are separate maintainer decisions with npm/GitHub policy limitations.
Nothing in this workflow performs those actions automatically.

## Troubleshooting

| Symptom | Check or recovery |
| --- | --- |
| OIDC ENEEDAUTH / E404 | Node/npm versions, exact owner/repository/workflow/environment fields, allowed direct-publish action, two-day expiry and GitHub-hosted runner |
| Waiting for approval | Configure/review `npm-release`; verify its `v*` tag deployment rule |
| Missing optional dependency | Install with optional dependencies enabled; confirm all five exact versions reached npm |
| Published integrity differs | Stop. Retain the reviewed bundle and inspect ownership/version history; never overwrite or accept different bytes |
| Registry scanning delay | Visibility checks are bounded; retry failed jobs with the preserved bundle after npm exposes the version |
| Conflicting launcher tarballs | Check LF checkout rules, Node/npm versions and package file modes; do not pick a conflicting copy silently |
| Native archive fails to execute | Check exact target, checksum, executable mode and OS runner; platform failure blocks publication |
| Draft download fails | Verify token contents access, tag, asset names and checksum sidecars; keep the draft unpublished |
| Final public download fails | Check unauthenticated asset visibility and the failing platform job; the release is not complete |
| Artifact expired | Restore the exact saved bundle through the reviewed process or use a new version; do not recreate different bytes for an existing npm version |

## Release verification checklist

- [ ] Tag/commit, Cargo workspace, launcher and five dependencies match exactly.
- [ ] All five target tests, Clippy, offline npm/archive checks and browser integration pass.
- [ ] Six package identities, native/npm binary agreement and all SHA checks pass.
- [ ] No missing, duplicate, unexpected or conflicting artifacts; licenses reviewed.
- [ ] Exact reviewed bundle retained with run ID and commit.
- [ ] Six npm names owned by the agreed maintainers; 2FA and trusted publisher setup reviewed.
- [ ] `npm-release` protections and explicit publication approval are in place.
- [ ] Five platform packages published/verified before the launcher.
- [ ] Five actual-registry installs pass version, help, SQUARE and native selection.
- [ ] Five actual draft archive installs pass without Node/Rust.
- [ ] Public GitHub release has five archives, five sidecars, aggregate checksums and manifest.
- [ ] All five final public npm and unauthenticated GitHub download checks pass.
- [ ] Update the README's unpublished notice only in a subsequent reviewed change.
