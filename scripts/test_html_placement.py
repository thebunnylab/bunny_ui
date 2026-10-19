"""Integrity regressions for the native initial-placement protocol."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("placement", Path(__file__).with_name("check-html-placement.py"))
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class PlacementTrace(unittest.TestCase):
    def test_each_initial_and_repeated_load_has_its_own_observation(self):
        source = "\n".join([
            "U 1.0 mount=1 attached=1 width=320.000 height=180.000",
            "V 1.1 mount=1 stage=load-request",
            "V 1.2 mount=1 stage=load-return",
            "U 2.0 mount=2 attached=1 width=450.000 height=260.000",
            "V 2.1 mount=2 stage=load-request",
            "U 3.0 mount=1 attached=1 width=640.000 height=360.000",
            "V 3.1 mount=1 stage=load-request",
        ])
        result = GATE.check(source)
        self.assertEqual(result["count"], 3)
        self.assertEqual(result["loads"][-1]["width"], 640)

    def test_detached_zero_size_and_unobserved_loads_are_refused(self):
        for observation in [
            "U 1.0 mount=1 attached=0 width=320.000 height=180.000",
            "U 1.0 mount=1 attached=1 width=0.000 height=180.000",
            "U 1.0 mount=1 attached=1 width=320.000 height=0.000",
            "",
        ]:
            with self.subTest(observation=observation), self.assertRaises(ValueError):
                GATE.check(observation + "\nV 1.1 mount=1 stage=load-request")

    def test_wrong_mount_stale_order_duplicate_private_fields_and_missing_load_refuse(self):
        good = "U 1.0 mount=1 attached=1 width=320.000 height=180.000"
        for source in [
            good + "\nV 1.1 mount=2 stage=load-request",
            good + "\nV 0.9 mount=1 stage=load-request",
            good + "\nV 1.1 mount=1 stage=load-request\nV 2.0 mount=1 stage=load-request",
            good + "\n" + good,
            good + " url=https://private.test\nV 1.1 mount=1 stage=load-request",
            good + "\nV 1.1 mount=1 stage=post\nV 1.2 mount=1 stage=load-request",
            good,
            "",
        ]:
            with self.subTest(source=source), self.assertRaises(ValueError):
                GATE.check(source)


if __name__ == "__main__":
    unittest.main()
