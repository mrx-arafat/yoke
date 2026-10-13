# Clean-room and license compliance

Yoke is an independent tool written from scratch.
This checklist keeps it that way and keeps it legally free to use, fork and redistribute.
It is engineering guidance, not legal advice; get an IP lawyer's review before any commercial launch.

## 1. Clean-room rules

- [ ] No source code, prompts, rule or skill text, config files, docs prose, images, logos or binaries from any commercial agent harness (including Pilot Shell) are copied, adapted or kept in this repository.
- [ ] No competitor binary is decompiled, disassembled or patched for this project.
- [ ] No license, activation or trial logic from another product is studied or reimplemented.
  Yoke ships no licensing code at all, and that must stay true.
- [ ] Known exposure: the maintainer has previously inspected Pilot Shell's license and trial internals.
  Because Yoke contains no licensing code, that exposure does not overlap with anything Yoke implements.
  Nothing from that earlier work (patches, notes, gists) is reused here.
- [ ] A contributor who has seen a competitor's private internals for a component does not write Yoke's version of that component.

## 2. Allowed inputs

- [ ] Public documentation of the agents Yoke integrates with, such as their published hook input and output formats.
  Matching a documented interface for interoperability is expected; copying the docs' wording is not.
- [ ] Ideas described publicly at concept level, such as "quality gates" or "persistent memory".
  Ideas are free to build on; specific expression (code, text, UI, file layouts) is not.
- [ ] Every outside source used during design is recorded in the PR description with its URL and what was taken from it.

## 3. Distinct name, UX and features

- [ ] The name, binary, commands, config files and directories never use "Pilot" or another product's marks.
  Yoke uses `yoke`, `yoke.toml` and `~/.yoke/`.
- [ ] Yoke does not mirror another product's command vocabulary, ports, file layout, colors, logo or screenshots.
  Workflow commands added later get Yoke's own names.
- [ ] The crates.io name `yoke` belongs to an unrelated crate, so the packages are `yoke-cli` and `yoke-core`.
  Confirm both are free with `cargo search` before the first publish.
- [ ] Run a trademark search for "Yoke" in software classes (USPTO and EUIPO, classes 9 and 42) before a public launch.

## 4. Dependency licenses

- [ ] Allowed: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, Unlicense.
- [ ] Not allowed: GPL, LGPL, AGPL, MPL, SSPL, BUSL, and anything unlicensed.
  MPL-2.0 is excluded on purpose to keep the graph fully permissive.
- [ ] The policy is enforced by `deny.toml`.
  Install the checker once with `cargo install cargo-deny --locked`, then run `cargo deny check licenses` locally and in CI.
- [ ] Zero-install spot check:
  `cargo metadata --format-version 1 | jq -r '.packages[] | select((.license // "NONE") | test("GPL|MPL|SSPL|BUSL|NONE")) | .name'`
  must print nothing.
- [ ] Every new dependency is checked against this list in the PR that adds it.

## 5. Shipping binaries

- [ ] MIT and Apache-2.0 require their notices to travel with the software, including compiled binaries.
  Each release bundles a `THIRD_PARTY_LICENSES` file generated from the dependency graph (for example with `cargo about`).
- [ ] Any `NOTICE` file shipped by an Apache-2.0 dependency is reproduced in that bundle.

## 6. Yoke's own license

- [ ] Yoke is licensed as `MIT OR Apache-2.0`, declared in `Cargo.toml` and backed by `LICENSE-MIT` and `LICENSE-APACHE`.
- [ ] Contributions are accepted under the same terms (inbound equals outbound).
  Contributors sign off commits with `git commit -s` (Developer Certificate of Origin).

## 7. Pre-release gate

1. `cargo deny check licenses` passes.
2. `THIRD_PARTY_LICENSES` is generated and included in the release artifacts.
3. Trademark and crates.io name checks are done and recorded.
4. A search of the repo for other products' names finds nothing beyond this file and comparisons in docs.
5. Every item in sections 1 to 6 is ticked.
