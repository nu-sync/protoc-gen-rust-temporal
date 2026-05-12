# Publishing to BSR as a curated plugin

`buf.build/nu-sync/rust-temporal` is the headline distribution path for
this plugin: users add it to their `buf.gen.yaml` and never install
anything locally. Getting there requires submitting a curated-plugin PR
to [`github.com/bufbuild/plugins`](https://github.com/bufbuild/plugins).

This document is the checklist.

## Why a PR, not a CLI push

Older buf releases shipped `buf alpha plugin push`, which let any user
push a self-hosted plugin image directly to BSR. That command was
retired (last available in buf ≤1.49 or so). Modern BSR-hosted codegen
plugins are *curated*: their Dockerfiles + manifests live in the
`bufbuild/plugins` monorepo, and Buf's CI builds the image and hosts it.

This is more bureaucratic but gives consumers a strong guarantee that
the plugin they pull from BSR was built from public, reviewable source.

## Submission checklist

1. **Tag a release on this repo** (this is what locks in the version
   the curated-plugin PR will reference). Already done for `v0.0.1` via
   `git tag v0.0.1 && git push origin v0.0.1`.
2. **Confirm the GitHub Release has prebuilt binaries** at
   `https://github.com/nu-sync/protoc-gen-rust-temporal/releases/tag/v0.0.1`.
   Buf's CI doesn't actually use the prebuilt binaries — it builds the
   image from the repo's Dockerfile — but having them available is a
   signal of release hygiene.
3. **Fork `github.com/bufbuild/plugins`**, then check out a branch:
   ```bash
   gh repo clone bufbuild/plugins
   cd plugins
   git checkout -b add-nu-sync-rust-temporal-v0.0.1
   ```
4. **Add the plugin directory:**
   ```
   plugins/community/nu-sync/protoc-gen-rust-temporal/v0.0.1/
   ├── Dockerfile          # copy from this repo's Dockerfile
   └── buf.plugin.yaml     # copy from this repo's buf.plugin.yaml, with
                           #   - source_url pinned to the v0.0.1 commit SHA
                           #   - plugin_version: v0.0.1 (matches dir)
                           #   - deps pointing at exact BSR module
                           #     versions (no :main floating refs)
   ```
5. **Run their test harness locally** to confirm the Dockerfile builds:
   ```bash
   make plugin name=protoc-gen-rust-temporal version=v0.0.1
   ```
   (See `bufbuild/plugins/README.md` for the exact commands — they
   evolve.)
6. **Open the PR.** Title: `Add nu-sync/protoc-gen-rust-temporal v0.0.1`.
   Body: link to the source repo's release, the SPEC.md, and the
   WIRE-FORMAT.md. Mention this is a Rust client generator paired with
   `protoc-gen-go-temporal` + (forthcoming) `protoc-gen-ts-temporal`.
7. **Address review comments.** Common asks:
   - Pin a specific Rust base image rather than `rust:slim`.
   - Use a `--no-cache` layer for the plugin binary to keep image sizes
     down.
   - Add `usage` / `description` polish to `buf.plugin.yaml`.
8. **After merge,** Buf's CI takes ~30 minutes to build and publish.
   Verify the plugin is live:
   ```bash
   buf generate --template <(echo 'version: v2
   plugins:
     - remote: buf.build/nu-sync/rust-temporal
       out: tmp')
   ```

## Bumping the plugin version later

Every new `vX.Y.Z` tag on this repo needs a fresh PR to
`bufbuild/plugins` adding `plugins/community/nu-sync/protoc-gen-rust-temporal/vX.Y.Z/`.
The directory copies forward — Buf's tooling does not auto-pick up new
releases.

A future automation could open that PR from a GitHub Action triggered
on release. For now it's manual; the cadence is expected to be slow.

## What stays in this repo

- The `Dockerfile` at the repo root — copied into the curated-plugin PR.
- The `buf.plugin.yaml` at the repo root — copied into the curated-plugin PR.
- This document.

The `release.yml` workflow used to include a `bsr-push` job invoking
`buf alpha plugin push`. That job was removed in favour of this manual
flow; see the comment block in `release.yml` where the job used to live.
