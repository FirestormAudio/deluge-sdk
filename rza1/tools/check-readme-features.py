#!/usr/bin/env python3
"""Every Cargo feature of each rza1 crate is named in that crate's README, and each README says
the crate needs a nightly toolchain.

A feature counts as named when the README has it in backticks (`name`). `default` is exempt,
and so are the features in HIDDEN, which are documented as hidden.

Usage: rza1/tools/check-readme-features.py   (from anywhere)
"""

import pathlib
import sys
import tomllib

HIDDEN = {"unstable-driver-steal"}
ROOT = pathlib.Path(__file__).resolve().parent.parent


def main():
    failed = False
    for manifest in sorted(ROOT.glob("*/Cargo.toml")):
        crate = manifest.parent
        features = tomllib.loads(manifest.read_text()).get("features", {})
        readme = crate / "README.md"
        if not readme.exists():
            print(f"error: {crate.name} has no README.md", file=sys.stderr)
            failed = True
            continue
        text = readme.read_text()
        if "nightly" not in text:
            print(f"error: {crate.name}/README.md does not say it needs a nightly toolchain", file=sys.stderr)
            failed = True
        for name in sorted(set(features) - {"default"} - HIDDEN):
            if f"`{name}`" not in text:
                print(f"error: {crate.name}/README.md does not name its feature `{name}`", file=sys.stderr)
                failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
