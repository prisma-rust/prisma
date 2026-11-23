import tempfile
import unittest
from pathlib import Path

from scripts import check_comments


class CheckCommentsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.old_root = check_comments.ROOT
        check_comments.ROOT = Path(self.tmp.name)

    def tearDown(self) -> None:
        check_comments.ROOT = self.old_root
        self.tmp.cleanup()

    def check(self, source: str) -> list[tuple[str, int, str]]:
        (check_comments.ROOT / "sample.rs").write_text(source, encoding="utf-8")
        return check_comments.check_file("sample.rs")

    def test_rustdoc_is_allowed(self) -> None:
        issues = self.check(
            "/// Returns the entity generation.\n"
            "///\n"
            "/// # Safety\n"
            "/// The caller must own the slot.\n"
            "pub unsafe fn generation() {}\n"
        )
        self.assertEqual(issues, [])

    def test_inline_comment_is_banned(self) -> None:
        issues = self.check("// Set the width.\nlet width = 1;\n")
        self.assertEqual([kind for kind, _, _ in issues], ["inline"])

    def test_block_comment_is_banned(self) -> None:
        issues = self.check("/* legacy */\nlet width = 1;\n")
        self.assertEqual([kind for kind, _, _ in issues], ["inline"])

    def test_todo_in_rustdoc_is_banned(self) -> None:
        issues = self.check("/// TODO(#123): replace this later.\npub fn f() {}\n")
        self.assertEqual([kind for kind, _, _ in issues], ["todo"])

    def test_cjk_in_rustdoc_is_banned(self) -> None:
        issues = self.check("/// 宽度\npub fn f() {}\n")
        self.assertEqual([kind for kind, _, _ in issues], ["cjk"])

    def test_fenced_diagram_separator_is_allowed(self) -> None:
        issues = self.check(
            "//! ```text\n"
            "//! A ---------------- B\n"
            "//! ```\n"
            "pub fn f() {}\n"
        )
        self.assertEqual(issues, [])

    def test_decorative_separator_in_rustdoc_is_banned(self) -> None:
        issues = self.check("/// ----------\npub fn f() {}\n")
        self.assertEqual([kind for kind, _, _ in issues], ["separator"])

    def test_suppression_rules(self) -> None:
        allow = self.check("#[allow(dead_code)]\nfn legacy() {}\n")
        expect = self.check(
            '#[expect(clippy::too_many_arguments, reason = "C ABI")]\npub fn f() {}\n'
        )
        self.assertEqual([kind for kind, _, _ in allow], ["allow"])
        self.assertEqual(expect, [])


if __name__ == "__main__":
    unittest.main()
