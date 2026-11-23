> 🤖 Created by <owner>'s AI agent (<model> / <tool>); not yet reviewed by a maintainer.

## Intent & State Change

- **Problem:** <one-sentence root cause>
- **Solution:** <one-sentence architectural / state change>

Fixes #<issue>   (remove this line if there is no issue)

## Verification Artifacts

- [ ] Static checks (`fmt`, `build`, `lint`, `clippy`): PASSED
- [ ] Automated / property tests: PASSED (`<command>`)
- **Runtime evidence:** <headless log, state snapshot, `PRISMA_BENCH_REPORT` JSON, `PRISMA_SCREENSHOT` PNG, or device + API level for Android>

Paste the full Markdown block from `python scripts/pr_ready.py` here, including `HEAD: <sha>` and
`**Result:**`. For comment-only PRs also paste `python scripts/check_comment_only.py --base origin/master`.

## Blast Radius & Risk

- **Modified modules:** `<list of directories>`
- **Breaking / core interface changes:** None | Requires Human Review
- **Refactor boundary:** None | <touched path + why the change cannot stay within the failing path (docs/code-style.md §5)>
- **Rollback:** `git revert <sha>`
