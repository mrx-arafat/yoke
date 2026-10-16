"""Focused tests for the Markdown documentation checker."""

import tempfile
import unittest
from pathlib import Path

from check_docs import check_files


def problems_for(files: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for name, text in files.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(text, encoding="utf-8")
        found = check_files(root, sorted(files))
    return [f"{rel}:{number}: {message}" for rel, number, message in found]


class CheckDocsTests(unittest.TestCase):
    def test_reports_broken_file_link_with_file_and_line(self) -> None:
        found = problems_for(
            {
                "docs/a.md": "ok\n[gone](missing.md) and [here](b.md)\n",
                "docs/b.md": "x\n",
            }
        )
        self.assertEqual(found, ["docs/a.md:2: broken link, no such file: missing.md"])

    def test_reports_broken_anchor_in_same_and_other_file(self) -> None:
        found = problems_for(
            {
                "a.md": "# Top\n[self](#nope) [other](b.md#also-nope)\n",
                "b.md": "# Real\n",
            }
        )
        self.assertEqual(
            found,
            [
                "a.md:2: missing anchor #nope in a.md",
                "a.md:2: missing anchor #also-nope in b.md",
            ],
        )

    def test_accepts_github_slugs_with_punctuation_and_duplicates(self) -> None:
        heading = "## Install & run: `yoke` (v2)!\n"
        links = "[1](#install--run-yoke-v2) [2](#install--run-yoke-v2-1)\n"
        self.assertEqual(problems_for({"a.md": heading + heading + links}), [])

    def test_reports_em_and_en_dashes(self) -> None:
        found = problems_for({"a.md": "fine\nbad \u2014 here\nand \u2013 here\n"})
        self.assertEqual(
            found,
            [
                "a.md:2: em dash U+2014 is not allowed",
                "a.md:3: en dash U+2013 is not allowed",
            ],
        )

    def test_ignores_links_in_code_blocks_code_spans_and_urls(self) -> None:
        text = (
            "```md\n[x](missing.md)\n```\n"
            "inline `[y](missing.md)` and [web](https://example.com/nope.md#frag)\n"
        )
        self.assertEqual(problems_for({"a.md": text}), [])


if __name__ == "__main__":
    unittest.main()
