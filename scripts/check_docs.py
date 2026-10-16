#!/usr/bin/env python3
"""Fail on broken links, missing anchors, and em or en dashes in tracked Markdown.

Standard library only. Checks the repository that contains this script and
prints one `file:line: message` per problem, then exits 1 if there were any.

Rules:
- A relative link must point at a file or directory that exists (exact case).
- A `#anchor` link into a Markdown file must match a heading slug (GitHub rules,
  duplicates numbered `-1`, `-2`) or an HTML `id`/`name` attribute.
- U+2014 and U+2013 are not allowed anywhere in tracked Markdown.
- http(s), mailto and other scheme links, fenced code blocks and code spans are ignored.
"""

import html
import os
import posixpath
import re
import subprocess
import sys
import unicodedata
from collections.abc import Iterator
from pathlib import Path
from urllib.parse import unquote

DASHES = {"\u2014": "em dash U+2014", "\u2013": "en dash U+2013"}

FENCE = re.compile(r"^\s*(`{3,}|~{3,})")
CODE_SPAN = re.compile(r"(?<!`)(`+)(?!`)(.+?)(?<!`)\1(?!`)")
ATX = re.compile(r"^ {0,3}#{1,6}(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$")
SETEXT = re.compile(r"^ {0,3}(?:=+|-+)[ \t]*$")
NOT_PARAGRAPH = re.compile(r"^\s*(?:[-*+]\s|\d+[.)]\s|>|\||#)")
HTML_ANCHOR = re.compile(r"<[A-Za-z][^>]*?\b(?:id|name)\s*=\s*[\"']([^\"']+)[\"']")
REF_DEFINITION = re.compile(r"^ {0,3}\[[^\]]+\]:[ \t]*(<[^>]*>|\S+)")
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.\-]*:")

Problem = tuple[str, int, str]


def prose_lines(text: str) -> Iterator[tuple[int, str]]:
    """Yield (line number, line) for every line outside fenced code blocks."""
    fence = ""
    for number, raw in enumerate(text.split("\n"), 1):
        line = raw.rstrip("\r")
        match = FENCE.match(line)
        if fence:
            stripped = line.strip()
            if stripped == fence[0] * len(stripped) and len(stripped) >= len(fence):
                fence = ""
        elif match:
            fence = match.group(1)
        else:
            yield number, line


def heading_text(raw: str) -> str:
    """Reduce inline Markdown in a heading to the plain text GitHub slugs."""
    spans: list[str] = []

    def stash(match: re.Match[str]) -> str:
        spans.append(match.group(2).strip())
        return f"\x00{len(spans) - 1}\x00"

    text = CODE_SPAN.sub(stash, raw)
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\[[^\]]*\]", r"\1", text)
    text = re.sub(r"<[^>]+>", "", text)
    text = re.sub(r"(?<!\w)(_+)(?=\S)(.+?)(?<=\S)\1(?!\w)", r"\2", text)
    text = html.unescape(text)
    text = re.sub(r"\\([!-/:-@\[-`{-~])", r"\1", text)
    return re.sub(r"\x00(\d+)\x00", lambda m: spans[int(m.group(1))], text)


def slugify(text: str) -> str:
    """GitHub heading slug: lowercase, drop punctuation except - and _, spaces to -."""
    kept: list[str] = []
    for char in text.strip().lower():
        category = unicodedata.category(char)
        if char in "-_ " or category[0] in "LMN" or category == "Pc":
            kept.append(char)
    return "".join(kept).replace(" ", "-")


def collect_anchors(text: str) -> set[str]:
    """Return every anchor a link into this Markdown text can use."""
    anchors: set[str] = set()
    occurrences: dict[str, int] = {}
    previous: str | None = None
    previous_number = 0
    for number, line in prose_lines(text):
        anchors.update(HTML_ANCHOR.findall(line))
        if number != previous_number + 1:
            previous = None
        previous_number = number
        atx = ATX.match(line)
        heading = None
        if atx:
            heading = atx.group(1)
        elif previous is not None and SETEXT.match(line):
            heading = previous
        paragraph = bool(line.strip()) and not atx and not NOT_PARAGRAPH.match(line)
        previous = line if paragraph else None
        if heading and heading.strip():
            slug = slugify(heading_text(heading))
            original = slug
            while slug in occurrences:
                occurrences[original] += 1
                slug = f"{original}-{occurrences[original]}"
            occurrences[slug] = 0
            anchors.add(slug)
    return anchors


def destinations(line: str) -> Iterator[str]:
    """Yield link destinations found in one line that has its code spans blanked."""
    for match in re.finditer(r"\]\(", line):
        index = match.end()
        while index < len(line) and line[index] in " \t":
            index += 1
        if index >= len(line):
            continue
        if line[index] == "<":
            end = line.find(">", index)
            dest = line[index + 1 : end] if end > 0 else ""
        else:
            depth = 0
            end = index
            while end < len(line):
                char = line[end]
                if char == "\\":
                    end += 2
                    continue
                if char in " \t":
                    break
                if char == "(":
                    depth += 1
                elif char == ")":
                    if depth == 0:
                        break
                    depth -= 1
                end += 1
            dest = line[index:end]
        if dest:
            yield re.sub(r"\\(.)", r"\1", dest)
    definition = REF_DEFINITION.match(line)
    if definition:
        yield definition.group(1).strip("<>")


class Checker:
    root: Path

    def __init__(self, root: Path) -> None:
        self.root = root
        self._listings: dict[Path, set[str]] = {}
        self._anchors: dict[str, set[str]] = {}

    def exists_exact(self, rel: str) -> bool:
        """True if rel exists under the root with exactly this letter case."""
        current = self.root
        for part in rel.split("/"):
            if part in ("", "."):
                continue
            if current not in self._listings:
                try:
                    self._listings[current] = set(os.listdir(current))
                except OSError:
                    return False
            if part not in self._listings[current]:
                return False
            current = current / part
        return True

    def anchors_of(self, rel: str) -> set[str]:
        if rel not in self._anchors:
            text = (self.root / rel).read_text(encoding="utf-8", errors="replace")
            self._anchors[rel] = collect_anchors(text)
        return self._anchors[rel]

    def check_link(self, source: str, dest: str) -> str | None:
        """Return a problem message for one destination, or None if it is fine."""
        if SCHEME.match(dest) or dest.startswith("//"):
            return None
        path, _, fragment = dest.partition("#")
        path = unquote(path.split("?", 1)[0])
        fragment = unquote(fragment)
        if not path:
            target = source
        else:
            base = "" if path.startswith("/") else posixpath.dirname(source)
            target = posixpath.normpath(posixpath.join(base, path.lstrip("/")))
            if target == ".." or target.startswith("../"):
                return f"link leaves the repository: {dest}"
            if not self.exists_exact(target):
                return f"broken link, no such file: {dest}"
        if fragment and target.endswith(".md") and (self.root / target).is_file():
            if fragment not in self.anchors_of(target):
                return f"missing anchor #{fragment} in {target}"
        return None

    def check_file(self, rel: str) -> list[Problem]:
        try:
            text = (self.root / rel).read_text(encoding="utf-8")
        except FileNotFoundError:
            return []
        except UnicodeDecodeError:
            return [(rel, 1, "not valid UTF-8")]
        problems: list[Problem] = []
        for number, line in enumerate(text.split("\n"), 1):
            for char, name in DASHES.items():
                if char in line:
                    problems.append((rel, number, f"{name} is not allowed"))
        for number, line in prose_lines(text):
            blanked = CODE_SPAN.sub(lambda m: " " * len(m.group(0)), line)
            for dest in destinations(blanked):
                message = self.check_link(rel, dest)
                if message:
                    problems.append((rel, number, message))
        return problems


def check_files(root: Path, files: list[str]) -> list[Problem]:
    checker = Checker(root)
    problems: list[Problem] = []
    for rel in files:
        problems.extend(checker.check_file(rel))
    return sorted(problems, key=lambda problem: (problem[0], problem[1]))


def tracked_markdown(root: Path) -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "*.md"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return sorted(name for name in out.split("\0") if name)


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    files = tracked_markdown(root)
    problems = check_files(root, files)
    for rel, number, message in problems:
        print(f"{rel}:{number}: {message}")
    if problems:
        broken = len({rel for rel, _, _ in problems})
        print(f"{len(problems)} problem(s) in {broken} file(s)", file=sys.stderr)
        return 1
    print(f"docs ok: {len(files)} Markdown files checked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
