import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import validate_feature_map


class FeatureMapTests(unittest.TestCase):
    def test_repo_feature_map_is_aligned_with_editor_sources(self) -> None:
        feature_map = json.loads(validate_feature_map.FEATURE_MAP_PATH.read_text(encoding="utf-8"))
        errors = validate_feature_map.validate(
            feature_map,
            validate_feature_map.APP_RS.read_text(encoding="utf-8"),
            validate_feature_map.KEYS_RS.read_text(encoding="utf-8"),
            validate_feature_map.UI_TREE_RS.read_text(encoding="utf-8"),
        )
        self.assertEqual(errors, [])

    def test_detects_stale_panel_or_command(self) -> None:
        bad_map = {
            "panels": {"nonExistentTab": "Nope"},
            "coreCommands": ["MissingCommand"],
            "nodePrefixes": [],
            "flows": [],
        }
        errors = validate_feature_map.validate(
            bad_map,
            validate_feature_map.APP_RS.read_text(encoding="utf-8"),
            validate_feature_map.KEYS_RS.read_text(encoding="utf-8"),
            validate_feature_map.UI_TREE_RS.read_text(encoding="utf-8"),
        )
        self.assertEqual(len(errors), 2)


if __name__ == "__main__":
    unittest.main()
