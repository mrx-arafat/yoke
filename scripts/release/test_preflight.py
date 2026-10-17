"""Focused preflight tests with a fake command runner: no git, gh, network or writes."""

import unittest

from preflight import (
    FAIL,
    NOT_FOUND,
    PASS,
    SKIP,
    Cmd,
    check_ci,
    check_clean_tree,
    check_tag,
)


class PreflightTests(unittest.TestCase):
    def test_tag_must_be_stable_and_match_the_cli_version(self) -> None:
        self.assertEqual(check_tag("v0.1.0", "0.1.0").status, PASS)
        # release.yml fires only on v-prefixed tags, so a bare version would never publish.
        for tag in ("0.1.0", "v0.2.0", "v0.1.0-rc1", "v0.1"):
            with self.subTest(tag=tag):
                self.assertEqual(check_tag(tag, "0.1.0").status, FAIL)

    def test_dirty_tree_fails_but_tooling_directories_are_ignored(self) -> None:
        def porcelain(output: str):
            return lambda args: Cmd(0, output, "")

        clean = check_clean_tree(porcelain("?? .codegraph/\n?? .lavish/\n"))
        self.assertEqual(clean.status, PASS)
        dirty = check_clean_tree(porcelain(" M Cargo.toml\n?? .lavish/\n"))
        self.assertEqual(dirty.status, FAIL)
        self.assertIn("Cargo.toml", dirty.detail)

    def test_missing_gh_is_skipped_not_passed(self) -> None:
        result = check_ci(
            lambda args: Cmd(NOT_FOUND, "", "gh: command not found"), "a" * 40
        )
        self.assertEqual(result.status, SKIP)
        self.assertIn("gh is not installed", result.detail)


if __name__ == "__main__":
    unittest.main()
