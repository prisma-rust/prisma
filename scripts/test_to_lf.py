import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import to_lf


class ToLfTests(unittest.TestCase):
    def test_rewrites_crlf_and_lone_cr(self) -> None:
        self.assertEqual(to_lf.convert_bytes(b"a\r\nb\rc\n"), b"a\nb\nc\n")

    def test_conversion_is_idempotent(self) -> None:
        once = to_lf.convert_bytes(b"a\r\nb\r\n")
        self.assertEqual(to_lf.convert_bytes(once), once)

    def test_treats_nul_byte_as_binary(self) -> None:
        self.assertTrue(to_lf.looks_binary(b"header\x00payload\r\n"))

    def test_convert_file_leaves_binary_untouched(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "blob.bin"
            original = b"\x00\x01\r\n"
            path.write_bytes(original)
            self.assertFalse(to_lf.convert_file(path))
            self.assertEqual(path.read_bytes(), original)

    def test_convert_file_rewrites_text(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "text.txt"
            path.write_bytes(b"one\r\ntwo\r\n")
            self.assertTrue(to_lf.convert_file(path))
            self.assertEqual(path.read_bytes(), b"one\ntwo\n")

    def test_convert_file_reports_unchanged_lf_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "text.txt"
            path.write_bytes(b"one\ntwo\n")
            self.assertFalse(to_lf.convert_file(path))
            self.assertEqual(path.read_bytes(), b"one\ntwo\n")


if __name__ == "__main__":
    unittest.main()
