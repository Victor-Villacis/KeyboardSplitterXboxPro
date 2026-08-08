# Releasing

A release is a pushed tag. There is nothing to click.

```sh
git switch master && git pull
# bump BOTH versions in the same commit (see below), then:
git tag v0.1.0
git push origin v0.1.0
```

That is it. `gh run watch` if you want to see it happen; a release appears on
the [releases page](https://github.com/Victor-Villacis/KeyboardSplitterXboxPro/releases)
with `ksx-0.1.0-setup.exe` attached, and `docs/FIRST-RUN.md` §1 moment 1 — "a
`.exe` from the releases page. One file." — is satisfied.

There is **no GitHub Release to create by hand first.** Creating one in the web
UI works only *because* it creates a tag; the tag is the trigger, so the UI step
is optional polish and never a requirement.

## The tag pattern

`v*`, declared in `.github/workflows/release.yml`. The same pattern the owner's
other repos release from, which is the reason it is `v*` and not something
cleverer.

The version part is not free text. `v<major>.<minor>.<patch>`, digits only: a
`v0.2.0-rc1` is refused in the first seconds of the run, because Inno Setup's
`VersionInfoVersion` is a numeric Windows version resource and cannot hold a
suffix. ksx has no prerelease channel.

## Two files must already say what the tag says

| file | field | what it becomes |
|---|---|---|
| `packaging/ksx.iss` | `#define AppVersion` | the installer's filename, its `VersionInfoVersion`, and the "ksx 0.1.0" row in Apps & Features |
| `Cargo.toml` | `[workspace.package] version` | what `ksx --version` prints |

`crates/ksx-app/tests/installer.rs` fails if those two disagree, so an ordinary
`cargo test` catches the common mistake (bumping one of them) long before a tag
exists. The release **also** checks the tag against both, before it builds
anything, and **fails rather than deriving** — see the long comment in
`.github/workflows/build-installer.yml` for why a version patched in by CI is
worse than a refused release.

If the tag was wrong, nothing has been built and nothing published:

```sh
git tag -d v0.1.0 && git push origin :refs/tags/v0.1.0
```

Fix the tree, commit, tag again. Do not reuse a version number that already has
a release: the tag is public the moment it is pushed.

## What the run does

`release.yml` calls `ci.yml` whole — fmt, clippy, all four feature
combinations, the test suite — and only then builds. A release cannot ship a
binary that skipped a check an ordinary branch push would have run. The build
itself is `build-installer.yml`, the same reusable workflow every branch push
uses, so the file a customer downloads is the file the gate proved rather than a
second build with the same command line.

Then it publishes: `gh release create` with the repository's own
`GITHUB_TOKEN` (no PAT, no secret to rotate, no third-party action), attaching

1. `ksx-<version>-setup.exe` — the file, and
2. `ksx.exe` — the bare program, for people who want no installer.

The release body comes from `packaging/release-notes.md` with the version, the
installer's name, its SHA-256 and the commit substituted in. **Edit the prose
there**, not in the workflow. Before publishing, the job re-hashes the
downloaded asset and refuses to publish if it does not match what the build
computed, so the SHA-256 on the page is provably the SHA-256 of the attached
file.

## Two gotchas, both cheap to hit

1. **`on: push: tags` runs the workflow file as it exists at the tagged
   commit.** A `release.yml` that only exists on a branch will not run. Tag
   master, after merging. The visible symptom of not having merged yet is that
   `gh workflow view release.yml` answers *"not found on the default branch"*
   and the Actions tab lists only CI — GitHub registers workflows from the
   default branch, so an unmerged release workflow is invisible AND inert.
2. **A tag pushed by `GITHUB_TOKEN` from inside Actions does not trigger
   workflows.** A tag you push from your machine does, which is the path above.

## SmartScreen, and why the release body talks about it

The installer is not code-signed, so Windows shows "Windows protected your PC"
with only a *Don't run* button visible. That is a statement about a certificate
this project has not bought, not a finding about the file — but a first-time
user meeting an unexplained warning stops there, and no later screen gets a
turn. So the release body names the dialog, gives the two clicks through it
(*More info* → *Run anyway*), says why plainly, and then gives the SHA-256 and
the commit so the file can be checked instead of trusted.

Signing it would remove the dialog and is the only thing that would. Until then
the honest paragraph is the product, and `crates/ksx-app/tests/installer.rs`
fails if it goes missing.
