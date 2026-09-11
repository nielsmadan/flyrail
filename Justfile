python := if os() == "windows" { "python" } else { "python3" }

default: check

check:
    {{python}} python/scripts/check.py check

format:
    {{python}} python/scripts/check.py format

sync:
    {{python}} python/scripts/check.py sync

lock:
    {{python}} python/scripts/check.py lock

build:
    {{python}} python/scripts/check.py build

package-check:
    {{python}} python/scripts/check.py package-check

dependency-audit:
    {{python}} python/scripts/check.py dependency-audit

workflow-check:
    {{python}} python/scripts/check.py workflow-check
