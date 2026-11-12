#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Writes debian/copyright (DEP-5) for Seeker and every crate it builds with.

Usage:
  packaging/gen-copyright.py [VENDOR_DIR] > debian/copyright

With VENDOR_DIR (the output of `cargo vendor`), crates are read from there and
each gets a `Files: vendor/<dir>/*` paragraph. Without it, `cargo metadata`
is used and paragraphs are keyed by crate name.
"""
import json
import os
import re
import subprocess
import sys
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
LIC_DIR = os.path.join(HERE, "licenses")

# SPDX id -> (DEP-5 short name, text file in packaging/licenses or None for common-licenses)
MAP = {
    "MIT": ("Expat", "MIT"),
    "Apache-2.0": ("Apache-2.0", None),
    "Apache-2.0 WITH LLVM-exception": ("Apache-2.0 with LLVM exception", "LLVM-exception"),
    "Zlib": ("Zlib", "Zlib"),
    "ISC": ("ISC", "ISC"),
    "0BSD": ("0BSD", "0BSD"),
    "BSD-2-Clause": ("BSD-2-clause", "BSD-2-Clause"),
    "BSD-3-Clause": ("BSD-3-clause", "BSD-3-Clause"),
    "BSL-1.0": ("BSL-1.0", "BSL-1.0"),
    "Unlicense": ("Unlicense", "Unlicense"),
    "Unicode-3.0": ("Unicode-3.0", "Unicode-3.0"),
    "OFL-1.1": ("OFL-1.1", "OFL-1.1"),
    "Ubuntu-font-1.0": ("Ubuntu-font-1.0", "Ubuntu-font-1.0"),
    "GPL-2.0-only": ("GPL-2", None),
    "GPL-2.0": ("GPL-2", None),
    "GPL-3.0-or-later": ("GPL-3+", None),
    "LGPL-2.1-or-later": ("LGPL-2.1+", None),
    "CC0-1.0": ("CC0-1.0", None),
}
COMMON = {
    "Apache-2.0": "Apache-2.0",
    "GPL-2": "GPL-2",
    "GPL-3+": "GPL-3",
    "LGPL-2.1+": "LGPL-2.1",
    "CC0-1.0": "CC0-1.0",
}


def convert(expr: str, used: set) -> str:
    """SPDX expression -> DEP-5 license field. Handles the shapes crates.io uses."""
    expr = re.sub(r"\s*/\s*", " OR ", expr.strip())  # old "MIT/Apache-2.0" style

    def term(t: str) -> str:
        t = t.strip()
        if t not in MAP:
            raise SystemExit(f"unknown license term {t!r}; add it to MAP")
        name = MAP[t][0]
        used.add(t)
        return name

    def disj(e: str) -> str:
        return " or ".join(term(x) for x in re.split(r"\s+OR\s+", e))

    m = re.fullmatch(r"\((.*)\)\s+AND\s+(.*)", expr)
    if m:  # "(A OR B) AND C [AND D]"
        rest = [disj(x) for x in re.split(r"\s+AND\s+", m.group(2))]
        return ", and ".join([disj(m.group(1))] + rest)
    if " AND " in expr:
        return " and ".join(disj(x) for x in re.split(r"\s+AND\s+", expr))
    return disj(expr)


def crates(vendor):
    if vendor:
        for d in sorted(os.listdir(vendor)):
            with open(os.path.join(vendor, d, "Cargo.toml"), "rb") as f:
                p = tomllib.load(f)["package"]
            yield f"vendor/{d}/*", p["name"], p["version"], p.get("authors", []), p["license"]
    else:
        m = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked"], cwd=os.path.dirname(HERE)))
        for p in sorted(m["packages"], key=lambda p: (p["name"], p["version"])):
            if p["source"] is None:
                continue
            yield (f"{p['name']}-{p['version']} (statically linked crate)", p["name"],
                   p["version"], p["authors"], p["license"])


def main():
    vendor = sys.argv[1] if len(sys.argv) > 1 else None
    used = {"GPL-3.0-or-later", "CC0-1.0"}
    out = [
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/",
        "Upstream-Name: seeker",
        "Upstream-Contact: Eric Allen <trajic@tutanota.com>",
        "Source: https://github.com/ElectricStars/seeker",
        "Comment: Seeker is statically linked with the Rust crates listed below.",
        "",
        "Files: *",
        "Copyright: 2026 Eric Allen",
        "License: GPL-3+",
        "",
        "Files: data/*.metainfo.xml",
        "Copyright: 2026 Eric Allen",
        "License: CC0-1.0",
        "",
        "Files: debian/*",
        "Copyright: 2026 Eric Allen <trajic@tutanota.com>",
        "License: GPL-3+",
        "",
    ]
    for files, name, ver, authors, lic in crates(vendor):
        holders = [re.sub(r"\s+", " ", a).strip() for a in authors if a.strip()]
        if not holders:
            holders = [f"The {name} developers"]
        out.append(f"Files: {files}")
        out.append("Copyright: " + "\n ".join(holders))
        out.append(f"License: {convert(lic, used)}")
        out.append(f"Comment: crate {name} {ver}")
        out.append("")
    names = {}
    for t in used:
        names[MAP[t][0]] = MAP[t][1]
    for dep5 in sorted(names):
        out.append(f"License: {dep5}")
        body = []
        if dep5 == "Apache-2.0 with LLVM exception":
            body.append("On Debian systems, the full text of the Apache License 2.0 can be found")
            body.append("in /usr/share/common-licenses/Apache-2.0. The exception reads:")
            body.append("")
        if dep5 in COMMON:
            body.append("On Debian systems, the full text of this license can be found in")
            body.append(f"/usr/share/common-licenses/{COMMON[dep5]}.")
        else:
            with open(os.path.join(LIC_DIR, names[dep5] + ".txt"), encoding="utf-8") as f:
                body += f.read().strip("\n").splitlines()
        for line in body:
            line = line.rstrip()
            out.append(" " + line if line.strip() else " .")
        out.append("")
    sys.stdout.write("\n".join(out).rstrip("\n") + "\n")


if __name__ == "__main__":
    main()
