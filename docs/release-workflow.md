# release workflow

how to cut a new ava release.

## pre-flight checks

before releasing, run locally with CI-equivalent strictness:

```bash
cargo fmt --all
cargo clippy
cargo test
```

all must pass — CI will reject the release otherwise.

## 1. review changes since last release

read every commit title **and description** — titles alone miss important details.

```bash
git log --format="%h %s%n%b---" $(git describe --tags --abbrev=0)..HEAD
```

## 2. check if README needs updating

review whether new features, changed install instructions, or removed capabilities require README updates. verify with the human before proceeding if unsure.

## 3. update changelog

edit `CHANGELOG.md`:

- move items from `[Unreleased]` to a new version section
- update the comparison links at the bottom

```markdown
## [Unreleased]

## [0.2.0]

### Added

- ...
```

## 4. bump version

update `version` in `Cargo.toml`:

```toml
version = "0.2.0"
```

## 5. update lockfile

run a build so `Cargo.lock` picks up the new version:

```bash
cargo build
```

## 6. commit the release

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release: v0.2.0"
```

## 7. ship to main (if in agent worktree)

```bash
brd agent merge
```

## 8. push and verify CI

push commits to main and **wait for CI to pass** before tagging:

```bash
git push origin main
```

check CI status with `hub ci-status -v` or at https://github.com/alextes/ava/actions — do not proceed until all checks pass.

## 9. tag and push

only after CI passes:

```bash
git tag v0.2.0
git push --tags
```

this triggers cargo-dist CI which builds binaries for all platforms and creates the github release.

## 10. publish to crates.io (currently skipped)

cargo publish is disabled for now — the binary assumes a cloned repo (DB path, upgrade command). install via `git clone` + `cargo install --path .` instead.
