# Version number specification

Applies to `prisma` repository product versions (engine, editor, launcher, build tools). User game project versions (`projects/*`), format versions, and ABI versions are **three separate things**, see section 5.

## 1. SemVer rules

Product versions are unified as `MAJOR.MINOR.PATCH`. Currently at `0.y.z` (current version `0.1`):

> **Zero-compatibility logic before 1.0**: Before 1.0 official release, **no compatibility logic is considered**, old version compatibility is explicitly regarded as redundant garbage code.
> Strictly prohibit writing any forward/backward compatibility layers, deprecation shims, compatibility facades, or old version data fallback branches. All changes directly replace/delete old implementations according to the latest optimal design.

| Change | Version field | Example |
|--------|--------------|---------|
| Breaking public game API, project module ABI, project file compatibility | `MINOR` (before 1.0) | `0.1.0 → 0.2.0` |
| New backward-compatible capabilities, internal refactoring, performance improvements | `PATCH` | `0.2.0 → 0.2.1` |
| Pure fix | `PATCH` | `0.2.1 → 0.2.2` |
| Breaking changes after 1.0 | `MAJOR` | `1.4.0 → 2.0.0` |

- Pre-release: `0.2.0-rc.1`, only for release candidates; `bump` drops pre-release suffix, formal release uses `python scripts/version.py set 0.2.0`.
- Build metadata (`+sha`) does not enter repository version; nightly doc site uses separate tags.
- `editor` and engine released at same version; Hub identifies editor version via `editor/Cargo.toml`, both must be consistent.
- 1.0.0 threshold is **public game API stability** (`prism-game-api` / `prism-project-api` no longer frequently breaking compatibility), not feature count.

## 2. Single version source

Single source: root `Cargo.toml` `[workspace.package].version`.

All crates in root workspace use `version.workspace = true`. Independent workspaces / frontend cannot inherit Cargo workspace version, synchronized by `scripts/version.py`:

| File | Field | Role |
|------|-------|------|
| `Cargo.toml` | `[workspace.package] version` | **Source** |
| `editor/Cargo.toml` | `[workspace.package] version` | Synced |
| `launcher/package.json` | `version` | Synced |
| `launcher/src-tauri/Cargo.toml` | `[package] version` | Synced |
| `launcher/src-tauri/tauri.conf.json` | `version` | Synced |
| `crates/xtask/Cargo.toml` | `[package] version` | Synced |
| `Cargo.lock` / `launcher/src-tauri/Cargo.lock` | package version entries | Refreshed by `cargo`, scripts do not manually edit |

Not participating in sync:

- `projects/*/Cargo.toml`: user game's own version;
- `tools/discord-bot/package.json`: independent tool;
- `prisma-hub`, `prisma-web-tutorial`: independent repositories;
- `launcher/src-tauri/gen/**`: generated artifacts (`tauri.properties` etc. untracked);
- `crates/prism-editor-mcp/Cargo.toml`: already uses `version.workspace = true`, no need to add to sync table.

## 3. Commands

```bash
python scripts/version.py show                  # print current version
python scripts/version.py check                 # verify all sync points
python scripts/version.py check --expect 0.2.0  # tag / release flow use
python scripts/version.py bump patch|minor|major
python scripts/version.py set 0.2.0
python scripts/version.py tag                   # print tag to hit: v0.2.0
```

- `--dry-run`: only print which files will be changed, not write.
- `--markdown`: output one line `| Version consistency | ✅/❌ | ... |` for `pr_ready.py` / CI use.
- `--quiet`: only output summary.
- `bump` / `set` update all sync points in section 2; after change run `cargo check` (or `cargo metadata`) to refresh `Cargo.lock`, and commit lock changes.
- **Do not manually edit** version numbers in sync points; `check` turns drift into red light.

## 4. Release process

1. Confirm CI all green on `master`.
2. Update `CHANGELOG.md` (version section + date + entries).
3. Run `python scripts/version.py bump <major|minor|patch>` (or `set X.Y.Z`).
4. Run `cargo check` to refresh `Cargo.lock`; confirm no unexpected dependency changes.
5. Commit: `chore(release): 0.2.0` (version number + CHANGELOG can be same commit).
6. Hit annotated tag: `git tag -a v0.2.0 -m "Prisma 0.2.0"`;
   `python scripts/version.py tag` prints correct tag.
7. Push branch and tag; create GitHub Release, release notes from `CHANGELOG.md`.
8. Doc site workflow automatically archives to `/v0.2.0/` based on tag (after site goes live; see section 6).

Tag rules:

- Format fixed `vMAJOR.MINOR.PATCH`, pre-release `v0.2.0-rc.1`.
- Tag points to CI-passed commit; `Cargo.toml` version on tag must match tag (`version.py check --expect`).
- Published stable tags are not moved or re-hit.

## 5. Format version and ABI version

These numbers **do not follow product version**:

| Version | Location | When +1 |
|---------|----------|---------|
| `ABI_VERSION` | `crates/prism-project-api` | Project module C ABI / `HostApi` table change |
| RSCN `version` | `prism-scene` | Scene binary layout / semantics change |
| RTTIBin `version` | `prism-reflect` | Self-describing binary format change |
| `.pak` version | `prism-asset` | Package container layout change |
| RTEX / RMES / RSKL / RSKN / RANI / RASM / RAGP | `prism-cooker` / `prism-asset` | Corresponding cooked asset format change |

Rules:

- Product version upgrade does **not automatically** change format / ABI version; format upgrade does not force product `MAJOR` change.
- **Zero-compatibility logic before 1.0**: Before 1.0, no fallback parsing code, shims, or compatibility facades are retained for old format or ABI; old data / old format parsing is directly deleted and replaced; old compatibility code is explicitly regarded as redundant garbage code.
- When breaking format compatibility: format version +1, update corresponding `docs/*.md`;
  0.y stage usually simultaneously upgrades product `MINOR`, letting users know they need to re-bake / re-export assets.
- When old data cannot be read, must explicitly error out; do not write silent degradation or fallback logic.

## 6. Doc site version directory

After site goes live, directory is automatically determined by version, not manually maintained:

- tag `vX.Y.Z` → `/vX.Y.Z/`;
- `master` → `/nightly/`;
- `/latest/` → latest stable tag;
- `versions.json` generated by scanning `meta.json` from each directory.

Product version only determines directory name and version label on page; API content is determined by `cargo doc` output of that tag.
Hosting method (GitHub Pages / Cloudflare / self-hosted) not yet decided, build script output is independent of hosting.

## 7. Automatic checks

- `scripts/pr_ready.py` always runs `version.py check` (fast, read-only).
- CI `lint` job runs `version.py check`, any sync point drift turns red light.
- Release / doc workflow uses `version.py check --expect "$VERSION"` to verify tag and manifest consistency.
- When `CHANGELOG.md` version section is inconsistent with `Cargo.toml`, release process should stop at local step 2.

## 8. FAQ

**Changed version but `Cargo.lock` didn't change?**

Path dependency version changes usually require `cargo check` to write back to lock; run corresponding command once in root and `launcher/src-tauri/`. Before commit, use `git status` to confirm lock changes match expectations.

**Why does independent workspace version drift?**

Cargo does not support cross-workspace version inheritance, only sync. `version.py check` is the only defense; when adding new independent workspace, add it to `TARGETS`, don't just write in docs.

**How to release pre-release?**

`set 0.2.0-rc.1` → hit `v0.2.0-rc.1` → after test passes `set 0.2.0` → hit `v0.2.0`.
`bump` is not used for "rc to formal" because it directly increments version field.
