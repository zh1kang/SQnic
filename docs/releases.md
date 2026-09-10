# releases

releases are created by `.github/workflows/release.yml` when a semver tag is
pushed. The tag must match the package version exactly, for example `v0.1.0`
for package version `0.1.0`.

each release contains versioned archives for:

- macOS arm64
- macOS x86_64
- Linux x86_64
- Windows x86_64

each archive contains one executable at the archive root. The release also
contains `SHA256SUMS` and a per-archive `.sha256` sidecar. The workflow runs
the extracted executable with `--version` and `--help`, then creates a database and round-trips an original Unicode record before publishing.
The packager verifies archive membership and its SHA-256 sidecar.
Normal CI also runs this archive smoke test on each runner platform.

to make an archive locally after a release build:

```sh
python3 scripts/package_release.py \
  --binary target/release/sqnic \
  --version 0.1.0 \
  --target aarch64-apple-darwin \
  --output-dir .artifacts --smoke-test
```

the workflow has read-only repository permissions during builds. Only the
publish job receives `contents: write`, and it runs for pushed version tags.

the model-free Pi callback check runs on Unix runners because its disposable
fixture is a Unix executable. Rust's automatic integration tests already guard
their Unix-only shell-command behavior with `cfg(unix)`; Windows still runs the
metadata, complete original-record pagination, remaining Rust tests and Python checks.

local verification does not prove that GitHub Actions or all four target binaries pass.
The workflow is prepared; no version tag or public release was pushed during this work.

release verification also enforces the [accuracy and latency gates](release-gates.md).
Saved live evidence must match current product and execution code and contain three passing candidate runs of each configured model.
Stale, incomplete or failed evidence blocks a tagged release.

## verification without publication

run the release workflow manually on a branch to check the saved live evidence, latency gates, and all four packaged targets.
Manual runs retain archives as workflow artifacts and do not publish a GitHub release.
Pull requests that change the release workflow or packager also run these checks.
The publish job accepts only pushed version tags.

```sh
gh workflow run release.yml --ref YOUR_BRANCH
```

See [native and long-history verification](remaining-verification.md) for the follow-up scope and limits.
