import tempfile
import unittest
from pathlib import Path

from scripts.check_arch_guards import check_c_abi_surface


class CheckArchGuardsTests(unittest.TestCase):
    def test_clean_c_abi_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "lib.rs"
            p.write_text("pub extern \"C\" fn tick(ptr: *mut u8) -> i32 { 0 }\n", encoding="utf-8")
            self.assertEqual(check_c_abi_surface(p), [])

    def test_forbidden_engine_or_dyn_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "lib.rs"
            p.write_text("pub fn bad(w: &World, d: &dyn Foo) {}\n", encoding="utf-8")
            self.assertEqual(len(check_c_abi_surface(p)), 1)


if __name__ == "__main__":
    unittest.main()
