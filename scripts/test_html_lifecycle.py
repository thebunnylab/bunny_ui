"""Regression checks for private, interleaved native HTML lifecycle records."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("html_lifecycle", Path(__file__).with_name("check-html-lifecycle.py"))
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class LifecycleTrace(unittest.TestCase):
    def test_interleaved_mounts_and_interrupted_navigation_remain_independent(self):
        result = GATE.check("\n".join([
            "V 1.0 mount=1 stage=load-request",
            "V 1.1 mount=1 stage=load-return",
            "V 1.2 mount=2 stage=load-request",
            "V 1.3 mount=2 stage=load-return",
            "V 1.4 mount=1 stage=commit",
            "V 1.5 mount=1 stage=post",
            "V 1.6 mount=1 stage=finish",
            "V 1.7 mount=2 stage=load-request",
            "V 1.8 mount=2 stage=load-return",
        ]))
        self.assertEqual(result["mounts"]["1"]["finish"], 1.6)
        self.assertNotIn("finish", result["mounts"]["2"])
        self.assertEqual(result["counts"]["load-request"], 3)

    def test_payloads_and_unknown_fields_are_refused(self):
        for line in [
            "V 1.0 mount=1 stage=post body=private-message",
            "V 1.0 mount=1 stage=post url=https://example.test",
            "V 1.0 mount=private-message stage=post",
            "V 1.0 mount=1 stage=private-message",
        ]:
            with self.subTest(line=line), self.assertRaises(ValueError):
                GATE.check(line)

    def test_empty_partial_or_impossible_callbacks_cannot_approve_a_witness(self):
        for source in ["", "V 1.0 mount=1 stage=finish", "V 1.0 mount=1 stage=load-request\nV 1.1 mount=1 stage=post", "V 1.0 mount=1 stage=load-request\nV 0.9 mount=1 stage=load-return"]:
            with self.subTest(source=source), self.assertRaises(ValueError):
                GATE.check(source)


if __name__ == "__main__":
    unittest.main()
