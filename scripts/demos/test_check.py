import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import check  # noqa: E402
import gen  # noqa: E402


class CheckTest(unittest.TestCase):
    def test_the_demos_match_the_app(self):
        self.assertEqual(check.missing(check.source(check.ROOT), gen.UI_TEXT), [])

    def test_a_changed_app_string_is_reported(self):
        src = check.source(check.ROOT).replace("stashed or unpushed", "or unpushed")
        stale = check.missing(src, gen.UI_TEXT)
        self.assertEqual(len(stale), 1)
        self.assertTrue(stale[0].startswith("git warning"))

    def test_the_notification_title_is_checked_where_it_is_sent(self):
        src = check.source(check.ROOT).replace('send("attic"', 'send("other"')
        self.assertEqual(len(check.missing(src, gen.UI_TEXT)), 1)

    def test_every_scene_builds(self):
        for name, scene in gen.SCENES.items():
            frames = scene()
            self.assertGreater(len(frames), 1, name)
            self.assertIn("<svg", gen.build(name, frames))


if __name__ == "__main__":
    unittest.main()
