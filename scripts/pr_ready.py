#!/usr/bin/env python3
"""Prisma pre-commit self-check: one command, one Markdown block for the PR body.

    python scripts/pr_ready.py                 # full self-check (default)
    python scripts/pr_ready.py --quick         # fast loop: fmt + clippy only
    python scripts/pr_ready.py --all           # whole workspace + every standalone workspace
    python scripts/pr_ready.py --base origin/master
    python scripts/pr_ready.py --verbose       # print the full output on failure

The checks are chosen from the diff against --base (including uncommitted and
untracked files):

  * rustfmt runs on **changed .rs files** only (pre-existing fmt drift is not charged to every PR);
  * changed .rs files are checked for comment style (warns during migration, see `docs/code-style.md`);
  * the root `Cargo.toml` and every standalone workspace must carry one version (`scripts/version.py`);
  * touched root-workspace crates: clippy --all-targets -D warnings + cargo test;
  * shader/reflection/bindings changes: re-run shader-bindgen into a temp dir and compare for drift;
  * editor/ changes: cargo test (standalone workspace, data layer + egui shell);
  * projects/* and launcher/ changes: cargo check per standalone workspace;
  * generated artifacts / signing material (.pak, keystore, ...) and whitespace errors are blocked (.spv is committed, never blocked).

stdout carries Markdown only; progress and failure details go to stderr. Any failed check exits 1.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IS_WINDOWS = os.name == "nt"
sys.path.insert(0, str(Path(__file__).resolve().parent))

from lib.console import force_utf8_streams  # noqa: E402

force_utf8_streams()

# A change to any of these files affects the whole root workspace.
WORKSPACE_WIDE = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
}
WORKSPACE_PREFIXES = (".cargo/",)

# CI's desktop job does not cover prism-app (it is built only as a user-project dependency).
# It is skipped locally too unless it changed itself or --all is passed; projects/template's check covers it.
SKIP_BY_DEFAULT = {"prism-app"}

BAD_SUFFIXES = (
    ".pak",
    ".pak.meta.json",
    ".keystore",
    ".jks",
    ".p12",
    ".pfx",
    ".pem",
    ".key",
)
# Whole path components, not substrings: legitimate paths such as
# `crates/prism-anim-retarget/` also contain `target`, and a substring test would
# flag a new crate as build output.
BAD_PARTS = ("node_modules", "target", "dist")
BAD_NAMES = {".env", "local.properties", "id_rsa", "id_ed25519"}


# --------------------------------------------------------------------------- #
# Infrastructure
# --------------------------------------------------------------------------- #

def eprint(*args: object) -> None:
    print(*args, file=sys.stderr, flush=True)


def run(cmd: list[str], cwd: Path = ROOT) -> tuple[int, str]:
    if cmd:
        # On Windows npm/pnpm/tsc are .cmd shims, and CreateProcess does not apply PATHEXT.
        resolved = shutil.which(cmd[0])
        if resolved:
            cmd = [resolved, *cmd[1:]]
    try:
        proc = subprocess.run(
            cmd,
            cwd=str(cwd),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError:
        return 127, f"command not found: {cmd[0]}"
    return proc.returncode, proc.stdout or ""


def git(*args: str) -> tuple[int, str]:
    return run(["git", *args])


def ensure_toolchain_in_path() -> None:
    """cargo may be missing from PATH when python is started from Explorer or a fresh shell."""
    cargo = shutil.which("cargo")
    if cargo:
        return
    candidates = [Path.home() / ".cargo" / "bin"]
    if IS_WINDOWS:
        candidates.append(Path(os.environ.get("USERPROFILE", "")) / ".cargo" / "bin")
    for candidate in candidates:
        exe = candidate / ("cargo.exe" if IS_WINDOWS else "cargo")
        if exe.exists():
            os.environ["PATH"] = str(candidate) + os.pathsep + os.environ.get("PATH", "")
            return


def resolve_base(explicit: str | None) -> str:
    if explicit:
        return explicit
    for ref in ("origin/master", "origin/main", "master", "main"):
        if git("rev-parse", "--verify", "--quiet", ref)[0] == 0:
            return ref
    return "HEAD"


def changed_files(base: str) -> list[str]:
    files: set[str] = set()
    probes = (
        ("diff", "--name-only", f"{base}...HEAD"),
        ("diff", "--name-only", "--cached"),
        ("diff", "--name-only"),
        ("ls-files", "--others", "--exclude-standard"),
    )
    for probe in probes:
        code, out = git(*probe)
        if code == 0:
            files.update(line.strip() for line in out.splitlines() if line.strip())
    return sorted(f.replace("\\", "/") for f in files if not f.startswith(".zcode/"))


def workspace_members() -> list[str]:
    try:
        import tomllib

        data = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        return [Path(member).name for member in data["workspace"]["members"]]
    except Exception:
        text = (ROOT / "Cargo.toml").read_text(encoding="utf-8", errors="replace")
        return re.findall(r'"crates/([^"/]+)"', text)


def crate_test_command(crate: str) -> list[str]:
    """Build a test command matching the targets actually declared by a crate.

    Some workspace members, such as `prism-editor-mcp`, are binary-only. Passing
    `--lib` unconditionally makes Cargo reject those packages before running any
    tests. Keep the original lib/bin/integration coverage, but only request the
    target kinds the package provides.
    """
    manifest = ROOT / "crates" / crate / "Cargo.toml"
    has_lib = (manifest.parent / "src" / "lib.rs").is_file()
    has_bin = (manifest.parent / "src" / "main.rs").is_file()
    try:
        import tomllib

        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        has_lib = has_lib or "lib" in data
        has_bin = has_bin or "bin" in data
    except (OSError, ValueError):
        pass

    command = ["cargo", "test", "-p", crate]
    if has_lib:
        command.append("--lib")
    if has_bin:
        command.append("--bins")
    command.append("--tests")
    if crate == "prism-render":
        # Every GPU integration test creates its own `VkInstance` / `VkDevice` in the
        # same binary. Parallel initialization intermittently SIGSEGVs on lavapipe
        # (#847), so serialize inside this binary; other crates keep the default
        # parallelism so the whole self-check does not slow down.
        command.extend(["--", "--test-threads=1"])
    return command


def has_prefix(paths: list[str], prefix: str) -> bool:
    return any(path.startswith(prefix) for path in paths)


def changed_rs(paths: list[str]) -> list[str]:
    return [p for p in paths if p.endswith(".rs") and (ROOT / p).is_file()]


def display_cmd(cmd: list[str], cwd: Path) -> str:
    rel = cwd.relative_to(ROOT) if cwd != ROOT else Path(".")
    prefix = "" if rel == Path(".") else f"(cd {rel.as_posix()} && "
    suffix = "" if rel == Path(".") else ")"
    return prefix + " ".join(cmd) + suffix


# --------------------------------------------------------------------------- #
# Checks
# --------------------------------------------------------------------------- #
class Check:
    def __init__(self, name: str, cmd: list[str], cwd: Path = ROOT, detail: str = "") -> None:
        self.name = name
        self.cmd = cmd
        self.cwd = cwd
        self.detail = detail
        self.status = "skip"
        self.output = ""
        self.done = False

    def run(self, verbose: bool) -> bool:
        self.done = True
        eprint(f"→ {self.name}: {display_cmd(self.cmd, self.cwd)}")
        code, out = run(self.cmd, self.cwd)
        self.output = out
        if code == 0:
            self.status = "ok"
            eprint("  ok")
            return True
        self.status = "fail"
        eprint(f"  FAILED (exit {code})")
        if "Traceback (most recent call last)" in out:
            # A crashed check is not a failing check; say so instead of leaving
            # the reader to guess why a green tree went red (#1019).
            eprint("  note: the check itself crashed (see the traceback below), not the code")
        if verbose:
            eprint(out)
        else:
            tail = "\n".join(out.rstrip().splitlines()[-25:])
            eprint(tail)
        return False


def test_summary(output: str) -> str:
    passed = sum(int(n) for n in re.findall(r"test result: ok\. (\d+) passed", output))
    failed = sum(int(n) for n in re.findall(r"test result: FAILED\. (\d+) passed", output))
    if failed:
        return f"{passed} passed, {failed} failed"
    return f"{passed} passed" if passed else "ok"


def file_matches_generated(generated: Path, checked_in: Path) -> bool:
    """Whether a generated file is equivalent to the committed one.

    bindgen always writes LF; on Windows `core.autocrlf=true` (the repo has no
    .gitattributes) turns the checked-out `.rs` into CRLF, so a byte comparison
    reports drift even when the **content is identical**. Compare bytes first
    (fast path), then normalize both sides to LF; real field / constant /
    ordering changes are still caught.
    """
    produced = generated.read_bytes()
    existing = checked_in.read_bytes()
    if produced == existing:
        return True
    return produced.replace(b"\r\n", b"\n") == existing.replace(b"\r\n", b"\n")


def shader_budget_check(changed: list[str]) -> Check:
    """Compare shader resource usage with the committed budget table."""
    check = Check("shader budget", ["shader-budget", "assets/shaders/budgets.json"])
    check.done = True
    touched = has_prefix(changed, "assets/shaders/") or has_prefix(changed, "crates/xtask/")
    if not touched:
        check.status = "skip"
        check.detail = "no shader changes"
        return check

    eprint("→ shader budget: count descriptors and push constants")
    code, out = run(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(ROOT / "crates" / "xtask" / "Cargo.toml"),
            "--bin",
            "shader-budget",
            "--",
            "--reflection-dir",
            str(ROOT / "assets" / "shaders" / "reflection"),
            "--budget",
            str(ROOT / "assets" / "shaders" / "budgets.json"),
        ]
    )
    check.output = out
    if code == 0:
        check.status = "ok"
        check.detail = out.strip().splitlines()[-1] if out.strip() else ""
        eprint("  ok")
        return check

    check.status = "fail"
    check.detail = "budget exceeded"
    eprint("  FAILED")
    eprint("\n".join(out.rstrip().splitlines()[-25:]))
    return check


def shader_bindings_check(changed: list[str]) -> Check:
    """Re-run bindgen into a temp dir and compare against the committed bindings; the worktree is untouched."""
    check = Check("shader bindings drift", ["shader-bindgen", "(temp out)"])
    check.done = True
    touched = has_prefix(changed, "assets/shaders/") or has_prefix(
        changed, "crates/prism-render/src/shader_bindings/"
    )
    if not touched:
        check.status = "skip"
        check.detail = "no shader changes"
        return check

    tmp = Path(tempfile.mkdtemp(prefix="prism-bindgen-"))
    try:
        eprint("→ shader bindings drift: regenerate into temp dir")
        code, out = run(
            [
                "cargo",
                "run",
                "--quiet",
                "--manifest-path",
                str(ROOT / "crates" / "xtask" / "Cargo.toml"),
                "--bin",
                "shader-bindgen",
                "--",
                str(ROOT / "assets" / "shaders" / "reflection"),
                str(tmp),
            ]
        )
        if code != 0:
            check.status = "fail"
            check.output = out
            check.detail = "bindgen failed"
            eprint("  FAILED (bindgen)")
            eprint("\n".join(out.rstrip().splitlines()[-25:]))
            return check

        committed = ROOT / "crates" / "prism-render" / "src" / "shader_bindings"
        diffs: list[str] = []
        crlf_only = 0
        generated = sorted(p for p in tmp.rglob("*") if p.is_file())
        for path in generated:
            rel = path.relative_to(tmp)
            current = committed / rel
            if not current.is_file():
                diffs.append(rel.as_posix())
                continue
            if file_matches_generated(path, current):
                if current.read_bytes() != path.read_bytes():
                    crlf_only += 1
                continue
            diffs.append(rel.as_posix())
        committed_names = {
            p.relative_to(committed).as_posix()
            for p in committed.rglob("*")
            if p.is_file()
        }
        generated_names = {p.relative_to(tmp).as_posix() for p in generated}
        diffs.extend(sorted(committed_names - generated_names))

        if diffs:
            check.status = "fail"
            check.detail = f"{len(diffs)} file(s) stale: {', '.join(diffs[:4])}"
            eprint(f"  FAILED: {check.detail}")
            eprint("  Run: bash assets/shaders/compile.sh && xtask shader-bindgen, then commit.")
        else:
            check.status = "ok"
            check.detail = "up to date"
            eprint("  ok")
            if crlf_only:
                eprint(
                    f"  note: {crlf_only} file(s) differ only in line endings "
                    "(CRLF checkout, LF generated); content matches."
                )
        return check
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def version_check() -> Check:
    """Version consistency (`scripts/version.py check`)."""
    return Check(
        "version sync",
        [sys.executable, str(ROOT / "scripts" / "version.py"), "check"],
    )


def dependency_budget_check() -> Check:
    """Player dependency budget (`scripts/check_deps.py`)."""
    return Check("dependency budget (player)", [sys.executable, str(ROOT / "scripts" / "check_deps.py")])


def feature_matrix_check() -> Check:
    """Modular feature matrix (`scripts/check_features.py`)."""
    return Check("feature matrix", [sys.executable, str(ROOT / "scripts" / "check_features.py")])


def standalone_lockfiles_check() -> Check:
    """Verify standalone workspace (`editor/`, `projects/*/`) `Cargo.lock` files are in sync."""
    manifests = [ROOT / "editor" / "Cargo.toml", *sorted((ROOT / "projects").glob("*/Cargo.toml"))]
    existing = [m for m in manifests if m.is_file() and m.with_name("Cargo.lock").is_file()]
    check = Check("standalone lockfiles", ["cargo", "metadata", "--locked"])
    check.done = True
    eprint("→ standalone lockfiles (--locked)")
    stale: list[str] = []
    for manifest in existing:
        res = subprocess.run(
            ["cargo", "metadata", "--manifest-path", str(manifest), "--locked", "--format-version", "1"],
            cwd=ROOT,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if res.returncode != 0:
            stale.append(str(manifest.relative_to(ROOT).parent))
    if stale:
        check.status = "fail"
        check.detail = f"stale Cargo.lock: {', '.join(stale)}"
        eprint(f"  FAILED: {check.detail}")
        return check
    check.status = "ok"
    check.detail = f"{len(existing)} workspaces"
    return check


def artifact_guard(changed: list[str]) -> Check:
    check = Check("artifact / secret guard", ["git", "diff --name-only"])
    check.done = True
    eprint("→ artifact / secret guard")
    bad: list[str] = []
    for path in changed:
        name = Path(path).name
        lowered = path.lower()
        if lowered.endswith(BAD_SUFFIXES) or name in BAD_NAMES:
            bad.append(path)
        elif any(part in BAD_PARTS for part in Path(path).parts):
            bad.append(path)
    if bad:
        check.status = "fail"
        check.detail = f"must not be committed: {', '.join(bad[:5])}"
        eprint(f"  FAILED: {check.detail}")
        return check
    check.status = "ok"
    check.detail = f"{len(changed)} file(s) checked"
    eprint("  ok")
    return check


def whitespace_check() -> Check:
    check = Check("whitespace / conflict markers", ["git", "diff", "--check"])
    check.done = True
    eprint("→ whitespace / conflict markers")
    for args in (("diff", "--check"), ("diff", "--cached", "--check")):
        code, out = git(*args)
        if code != 0:
            check.status = "fail"
            check.output = out
            check.detail = out.strip().splitlines()[0] if out.strip() else "diff --check failed"
            eprint(f"  FAILED: {check.detail}")
            return check
    check.status = "ok"
    check.detail = "clean"
    eprint("  ok")
    return check


def format_check(files: list[str], verbose: bool) -> Check:
    check = Check("format (changed .rs)", ["rustfmt", "--check"])
    check.done = True
    eprint("→ format (changed .rs)")
    if not files:
        check.status = "skip"
        check.detail = "no .rs changes"
        return check
    rustfmt = shutil.which("rustfmt")
    if not rustfmt:
        check.status = "skip"
        check.detail = "rustfmt not on PATH"
        eprint("  skipped: rustfmt not on PATH")
        return check

    failures: list[str] = []
    for path in files:
        cwd = ROOT
        code, out = run([rustfmt, "--edition", "2021", "--check", str(ROOT / path)], cwd)
        if code != 0:
            failures.append(path)
            if verbose:
                eprint(out)
    if failures:
        check.status = "fail"
        check.detail = f"{len(failures)} file(s): {', '.join(failures[:4])}"
        eprint(f"  FAILED: {check.detail}")
        eprint("  Run: cargo fmt --all  (or rustfmt on the listed files)")
    else:
        check.status = "ok"
        check.detail = f"{len(files)} file(s)"
        eprint("  ok")
    return check


def comment_check(base: str) -> Check:
    """Comment style check (`scripts/check_comments.py --added-lines`)."""
    check = Check(
        "comment style",
        [
            sys.executable,
            str(ROOT / "scripts" / "check_comments.py"),
            "--changed",
            "--base",
            base,
            "--added-lines",
        ],
    )
    check.done = True
    eprint("→ comment style: added/modified .rs lines")
    code, out = run(check.cmd)
    check.output = out
    if code != 0:
        check.status = "fail"
        match = re.search(r"comments: (\d+) finding\(s\)", out)
        count = int(match.group(1)) if match else 0
        check.detail = f"{count} new violation(s)" if count else "check_comments.py failed"
        eprint("  FAILED")
        eprint("\n".join(out.rstrip().splitlines()[-10:]))
        return check

    check.status = "ok"
    check.detail = "clean"
    eprint("  ok")
    return check


def editor_checks(changed: list[str], quick: bool) -> list[Check]:
    """`editor/` is a standalone workspace (egui + ash) that only runs on a desktop with Vulkan.

    On Termux only a type check is possible: it links `prism-render` / `prism-engine`,
    so rendering / engine changes in the root workspace re-check the editor as well.
    """
    manifest = ROOT / "editor" / "Cargo.toml"
    tests = Check(
        "editor tests",
        ["cargo", "test", "--manifest-path", str(manifest)],
    )
    check = Check(
        "editor check",
        [
            "cargo",
            "check",
            "--manifest-path",
            str(manifest),
            "--target",
            "x86_64-pc-windows-gnu",
        ],
    )

    touched = (
        has_prefix(changed, "editor/")
        or has_prefix(changed, "crates/prism-render/")
        or has_prefix(changed, "crates/prism-engine/")
        or has_prefix(changed, "crates/prism-scene/")
    )
    if not touched:
        for item in (tests, check):
            item.status = "skip"
            item.detail = "editor untouched"
            item.done = True
        return [tests, check]

    code, installed = run(["rustup", "target", "list", "--installed"])
    if code != 0 or "x86_64-pc-windows-gnu" not in installed:
        check.status = "skip"
        check.detail = "rustup target x86_64-pc-windows-gnu is not installed"
        check.done = True
    if quick:
        for item in (tests, check):
            item.status = "skip"
            item.detail = "quick mode"
            item.done = True
    return [tests, check]


# --------------------------------------------------------------------------- #
# Main flow
# --------------------------------------------------------------------------- #
def main() -> int:
    parser = argparse.ArgumentParser(description="Prisma pre-PR self-check")
    parser.add_argument("--base", help="comparison base (defaults to origin/master)")
    parser.add_argument("--quick", action="store_true", help="fmt + clippy only, no tests")
    parser.add_argument(
        "--static-only",
        action="store_true",
        help="static/script gates only (skips cargo clippy and tests; for Termux or docs/CI PRs)",
    )
    parser.add_argument("--all", action="store_true", help="check every workspace and standalone project")
    parser.add_argument("--verbose", action="store_true", help="print the full output on failure")
    args = parser.parse_args()

    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")

    ensure_toolchain_in_path()
    base = resolve_base(args.base)
    changed = changed_files(base)
    rs_files = changed_rs(changed)

    members = workspace_members()
    crates: set[str] = set()
    if args.all:
        crates = set(members)
    else:
        for path in changed:
            if path.startswith("crates/"):
                parts = path.split("/")
                if len(parts) > 1 and parts[1] in members:
                    crates.add(parts[1])
        if any(p in WORKSPACE_WIDE or p.startswith(WORKSPACE_PREFIXES) for p in changed):
            crates = set(members)

    # `prism-app` is skipped by default only because checking it is expensive and
    # CI does not cover it; when it **itself** changed it must run, otherwise a
    # `prism-app` self-check would report "touched crates: (none)".
    directly_changed = {
        path.split("/")[1]
        for path in changed
        if path.startswith("crates/") and len(path.split("/")) > 1
    }
    if args.all:
        default_crates = sorted(crates)
    else:
        default_crates = sorted(crates - (SKIP_BY_DEFAULT - directly_changed))
    eprint(f"Prisma self-check — base {base}, {len(changed)} changed file(s)")
    eprint(f"  touched crates: {', '.join(default_crates) or '(none)'}")

    checks: list[Check] = [whitespace_check(), artifact_guard(changed)]
    checks.append(format_check(rs_files, args.verbose))
    checks.append(comment_check(base))
    checks.append(version_check())

    if default_crates and not args.static_only:
        clippy_cmd = ["cargo", "clippy"]
        for crate in default_crates:
            clippy_cmd += ["-p", crate]
        clippy_cmd += ["--all-targets", "--", "-D", "warnings"]
        checks.append(Check(f"clippy ({len(default_crates)} crate{'s' if len(default_crates) > 1 else ''})", clippy_cmd))

        if not args.quick:
            # Run each package separately because Cargo applies `--lib` and
            # `--bins` to the whole selection. A binary-only package must not
            # inherit `--lib` from a neighboring library package.
            for crate in default_crates:
                checks.append(Check(f"test ({crate})", crate_test_command(crate)))

    if args.all or has_prefix(changed, "crates/xtask/"):
        checks.append(
            Check(
                "xtask check",
                ["cargo", "check", "--manifest-path", str(ROOT / "crates" / "xtask" / "Cargo.toml")],
            )
        )

    checks.append(shader_bindings_check(changed))
    checks.append(shader_budget_check(changed))
    checks.append(dependency_budget_check())
    if args.all or any(path.endswith("Cargo.toml") for path in changed):
        checks.append(feature_matrix_check())

    checks.extend(editor_checks(changed, args.quick))

    if not args.quick:
        standalone = [
            ("projects/template", "projects/template/"),
            ("projects/fishing", "projects/fishing/"),
            ("projects/sponza", "projects/sponza/"),
            ("launcher", "launcher/"),
        ]
        for name, prefix in standalone:
            manifest = ROOT / name / "Cargo.toml"
            if manifest.is_file() and (args.all or has_prefix(changed, prefix)):
                checks.append(
                    Check(
                        f"{name} check",
                        ["cargo", "check", "--manifest-path", str(manifest)],
                    )
                )

    if args.all or has_prefix(changed, "docs/") or has_prefix(changed, "scripts/"):
        checks.append(
            Check(
                "docs naming & index",
                [sys.executable, str(ROOT / "scripts" / "check_docs.py")],
            )
        )
    if args.all or has_prefix(changed, "assets/shaders/"):
        if shutil.which("slangc"):
            checks.append(
                Check(
                    "SPIR-V freshness",
                    ["bash", str(ROOT / "assets" / "shaders" / "compile.sh")],
                )
            )
        else:
            spv_skip = Check("SPIR-V freshness", ["slangc"])
            spv_skip.status = "skip"
            spv_skip.detail = "slangc not on PATH"
            spv_skip.done = True
            checks.append(spv_skip)
    if args.all or has_prefix(changed, "crates/") or has_prefix(changed, "scripts/"):
        checks.append(
            Check(
                "arch guards",
                [sys.executable, str(ROOT / "scripts" / "check_arch_guards.py")],
            )
        )
    if args.all or has_prefix(changed, "crates/") or has_prefix(changed, "projects/") or has_prefix(changed, "editor/") or any(Path(p).name in ("Cargo.toml", "Cargo.lock") for p in changed):
        checks.append(standalone_lockfiles_check())
    if args.all or has_prefix(changed, "editor/") or "docs/feature-map.json" in changed or has_prefix(changed, "scripts/"):
        checks.append(
            Check(
                "feature map",
                [sys.executable, str(ROOT / "scripts" / "validate_feature_map.py")],
            )
        )
    if args.all or has_prefix(changed, "scripts/"):
        checks.append(
            Check(
                "scripts tests",
                [
                    sys.executable,
                    "-m",
                    "unittest",
                    "discover",
                    "-s",
                    str(ROOT / "scripts"),
                    "-p",
                    "test_*.py",
                ],
            )
        )

    failed = False
    for check in checks:
        if check.done:
            continue
        if not check.run(args.verbose):
            failed = True
    if not failed:
        failed = any(check.status == "fail" for check in checks)

    # Some checks only know their detail after they have run.
    for check in checks:
        if check.name.startswith("test (") and check.status == "ok":
            check.detail = test_summary(check.output)
        elif check.name == "editor check" and check.status == "ok":
            check.detail = "x86_64-pc-windows-gnu"
        elif check.name == "editor tests" and check.status == "ok":
            check.detail = test_summary(check.output)

        elif check.name.startswith("feature matrix") and check.status == "ok":
            check.detail = check.output.strip().split("|")[-2].strip()

        elif check.name.startswith("dependency budget") and check.status == "ok":
            match = re.search(r"player (\d+)/(\d+) crates", check.output)
            check.detail = f"{match.group(1)}/{match.group(2)} crates" if match else ""

        elif check.name == "version sync" and check.status == "ok":
            match = re.search(r"version: (\S+)", check.output)
            check.detail = match.group(1) if match else ""

        elif check.name.startswith("clippy (") and check.status == "ok":
            check.detail = "-D warnings"

    behind = 0
    behind_code, behind_out = git("rev-list", "--count", f"HEAD..{base}")
    if behind_code == 0 and behind_out.strip().isdigit():
        behind = int(behind_out.strip())
    if behind:
        eprint(
            f"  ⚠ {behind} commit(s) behind {base} — run `git rebase {base}` locally before pushing"
        )

    sha_code, sha_out = git("rev-parse", "--short", "HEAD")
    sha = sha_out.strip() if sha_code == 0 else "unknown"
    dirty = bool(git("status", "--porcelain")[1].strip())
    mode = (
        "static-only"
        if args.static_only
        else ("quick" if args.quick else ("all" if args.all else "full"))
    )

    lines = [
        "## Self-check (`scripts/pr_ready.py`)",
        "",
        f"- mode: **{mode}** · base: `{base}`"
        + (f" (**{behind} commit(s) behind, not rebased**)" if behind else "")
        + f" · HEAD: `{sha}`"
        + (" · working tree dirty" if dirty else "")
        + f" · changed: {len(changed)} file(s)",
        f"- crates: {', '.join(f'`{c}`' for c in default_crates) or '—'}",
        "",
        "| Check | Result | Detail |",
        "|---|---|---|",
    ]
    icons = {"ok": "✅", "fail": "❌", "skip": "⏭️", "warn": "⚠️"}
    for check in checks:
        detail = check.detail.replace("|", "\\|")
        lines.append(f"| {check.name} | {icons[check.status]} | {detail} |")
    lines.append("")
    if failed:
        lines.append("**Result: ❌ checks failed**")
    elif any(check.status == "warn" for check in checks):
        lines.append("**Result: ✅ all checks passed** (comment style warnings only)")
    else:
        lines.append("**Result: ✅ all checks passed**")

    print("\n".join(lines))
    sys.stdout.flush()

    if failed and not shutil.which("slangc"):
        eprint("")
        eprint("Note: slangc is not installed locally. assets/shaders/*.spv are committed;")
        eprint("      restore clean workspace or run `bash assets/shaders/compile.sh` if missing.")

    eprint("")
    eprint("PASS" if not failed else "FAIL")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
