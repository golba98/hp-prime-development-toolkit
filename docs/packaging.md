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
declared platform. The artifact workflow prepares packages and archives; it
does not publish npm packages or create GitHub releases. Native users may copy
the extracted binary onto PATH without Node or Rust.

CI runner labels follow the [GitHub-hosted runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
