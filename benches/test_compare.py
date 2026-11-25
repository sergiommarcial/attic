import pathlib
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import compare  # noqa: E402


class CompareTest(unittest.TestCase):
    def test_median_and_p90(self):
        values = [5, 1, 3, 2, 4, 9, 7, 8, 6, 10]
        self.assertEqual(compare.median(values), 5.5)
        self.assertEqual(compare.p90(values), 10)

    def test_times_are_printed_the_way_the_readme_prints_them(self):
        self.assertEqual(compare.fmt_ms(0.42), "0.4 ms")
        self.assertEqual(compare.fmt_ms(4.4), "4.4 ms")
        self.assertEqual(compare.fmt_ms(329.2), "329 ms")
        self.assertEqual(compare.fmt_ms(2100), "2.1 s")

    def test_a_run_line_is_parsed(self):
        self.assertEqual(compare.parse_run("146000 158.20 1240.50"), ("146000", 158.2, 1240.5))

    def test_a_scenario_list_is_parsed(self):
        text = "typical\tproject\tproject, 3k files\nstress\twide\tone folder, 100k files\n"
        self.assertEqual(
            compare.parse_list(text),
            [("typical", "project", "project, 3k files"), ("stress", "wide", "one folder, 100k files")],
        )

    def test_a_comparison_table_marks_typical_and_stress_rows(self):
        rows = [
            {"kind": "typical", "what": "project", "base": 4.4, "new": 2.7},
            {"kind": "stress", "what": "wide", "base": 319.0, "new": 315.0},
        ]
        out = compare.table(rows, "origin/main").splitlines()
        self.assertEqual(out[0], "| Scenario | origin/main | new | change |")
        self.assertEqual(out[2], "| **Typical:** project | 4.4 ms | 2.7 ms | x0.61 |")
        self.assertEqual(out[3], "| Stress: wide | 319 ms | 315 ms | x0.99 |")

    def test_a_table_without_a_base_has_one_column_of_numbers(self):
        out = compare.table([{"kind": "typical", "what": "project", "new": 2.7}]).splitlines()
        self.assertEqual(out[0], "| Scenario | median |")
        self.assertEqual(out[2], "| **Typical:** project | 2.7 ms |")

    def test_a_base_at_the_noise_floor_has_no_ratio(self):
        for base in (0, 0.1):
            out = compare.table([{"kind": "stress", "what": "x", "base": base, "new": 14.0}], "b")
            self.assertIn("| n/a |", out)
            self.assertNotIn("x140", out)

    def test_a_base_just_above_the_floor_has_a_ratio(self):
        out = compare.table([{"kind": "stress", "what": "x", "base": 0.5, "new": 1.0}], "b")
        self.assertIn("x2.00", out)


class LockTest(unittest.TestCase):
    def tree(self, tmp):
        root = pathlib.Path(tmp)
        (root / "benches").mkdir()
        (root / "Cargo.lock").write_text("root lock\n")
        (root / "benches" / "Cargo.lock").write_text("stale lock\n")
        return root

    def test_the_benches_lock_is_made_from_the_root_lock(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(compare.subprocess, "run") as run:
            root = self.tree(tmp)
            compare.sync_lock(root)
            self.assertEqual((root / "benches" / "Cargo.lock").read_text(), "root lock\n")
            cmd = run.call_args.args[0]
            self.assertEqual(cmd[:2], ["cargo", "metadata"])
            self.assertIn(str(root / "benches/Cargo.toml"), cmd)

    def test_a_build_syncs_the_lock_before_cargo_builds(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(compare.subprocess, "run") as run:
            root = self.tree(tmp)
            compare.build(root, root / "target")
            self.assertEqual([c.args[0][1] for c in run.call_args_list], ["metadata", "build"])
            self.assertEqual((root / "benches" / "Cargo.lock").read_text(), "root lock\n")


if __name__ == "__main__":
    unittest.main()
