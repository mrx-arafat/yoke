<!--
Thanks for contributing to Yoke.
Read CONTRIBUTING.md and COMPLIANCE.md before opening this pull request.
Fill in every section below. Delete the HTML comments when you are done.
-->

## Summary

<!-- One or two sentences: what this pull request changes and why. -->

## Linked issue

<!-- Link the issue this pull request closes, for example `Closes #123`. -->
<!-- If there is no issue, say why not. -->

Closes #

## Checks

<!-- Run the checks from CONTRIBUTING.md that cover your change and paste the result. -->
<!-- At minimum, run the commands below from the repository root. -->

```sh
cargo build --workspace --locked
cargo fmt --all -- --check
```

<!-- Then run the smallest existing tests covering the change, for example: -->

```sh
cargo test -p yoke-core config::tests -- --quiet
```

Paste the commands you ran and their results:

```
```

## Documentation

<!-- User guides must match the code in the same change. -->
<!-- List the docs you updated, or state "no doc impact" with a one-line reason. -->

Docs updated: <!-- e.g. docs/workflows.md -->

## Changelog

<!-- CONTRIBUTING.md: do not edit CHANGELOG.md directly. -->
<!-- When your change is user-visible, add a reviewed record under docs/changes/ and
     regenerate with:
       uv run --no-project python scripts/changelog.py
       uv run --no-project python scripts/changelog.py --check
     When your change is not user-visible (for example an internal refactor or test
     fix), state "no changelog entry needed" with a one-line reason. -->

Changelog record: <!-- e.g. docs/changes/0.1.0.json, or "no changelog entry needed" -->

## License and compliance

<!-- COMPLIANCE.md section 6: contributions are accepted under MIT OR Apache-2.0,
     inbound equals outbound, and commits are signed off with `git commit -s`. -->

- [ ] My commits are signed off (`git commit -s`, Developer Certificate of Origin).
- [ ] My change does not copy source, prompts, config, docs prose, images, or binaries from any commercial agent harness.
- [ ] Every new dependency is allowed by `deny.toml` (MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, Unlicense).
- [ ] Every outside source used during design is recorded in the summary with its URL and what was taken from it.
