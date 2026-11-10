#!/usr/bin/env python3
"""Turns the NamePtr indices in a profiled nanoda log (D-372) into declaration names.

Usage: annotate-nanoda-log.py EXPORT LOG

EXPORT is the export file nanoda checked. Its `#NS` and `#NI` lines define the name ids,
and nanoda's NamePtr index is that id. Each `Ptr { idx: N, ... }` in LOG is replaced by
the dotted name, and the slow-declaration lines are listed by time at the end.
"""
import re
import sys


def load_names(path):
    names = {}
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            parts = line.split()
            if len(parts) >= 4 and parts[1] == "#NS":
                names[int(parts[0])] = (int(parts[2]), parts[3])
            elif len(parts) >= 4 and parts[1] == "#NI":
                names[int(parts[0])] = (int(parts[2]), parts[3])

    cache = {0: ""}

    def resolve(i):
        if i in cache:
            return cache[i]
        if i not in names:
            return f"?{i}"
        pfx, sfx = names[i]
        base = resolve(pfx)
        cache[i] = sfx if base == "" else f"{base}.{sfx}"
        return cache[i]

    return resolve


def main():
    export, log = sys.argv[1], sys.argv[2]
    resolve = load_names(export)
    ptr = re.compile(r"Ptr \{ idx: (\d+)[^}]*\}")
    slow = []
    out = []
    with open(log, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = ptr.sub(lambda m: resolve(int(m.group(1))), line)
            if "DIAG slow" in line:
                m = re.search(r"ms=(\d+)", line)
                if m:
                    slow.append((int(m.group(1)), line.rstrip()))
            out.append(line)
    with open(log + ".named", "w", encoding="utf-8") as f:
        f.writelines(out)
    slow.sort(reverse=True)
    with open(log + ".slowest", "w", encoding="utf-8") as f:
        for ms, line in slow[:200]:
            f.write(line + "\n")
    print(f"named {len(out)} lines; {len(slow)} declarations took at least 1 s")
    for ms, line in slow[:25]:
        print(line)


if __name__ == "__main__":
    main()
