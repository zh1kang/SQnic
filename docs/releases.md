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
the packaged binary with `--version` and `--help` before publishing.

to make an archive locally after a release build:

```sh
python3 scripts/package_release.py \
  --binary target/release/sqnic \
  --version 0.1.0 \
  --target aarch64-apple-darwin \
  --output-dir .artifacts
```

the workflow has read-only repository permissions during builds. Only the
publish job receives `contents: write`, and it runs for pushed version tags.

the model-free Pi callback check runs on Unix runners because its disposable
fixture is a Unix executable. Rust's automatic integration tests already guard
their Unix-only process behavior with `cfg(unix)`; Windows still runs the
remaining Rust tests and the Python checks.

local verification does not prove that GitHub Actions or all four target binaries pass.
The workflow is prepared; no version tag or public release was pushed during this work.
