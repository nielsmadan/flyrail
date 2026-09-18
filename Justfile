python := if os() == "windows" { "python" } else { "python3" }

default: check

setup:
    {{python}} python/scripts/check.py sync
    lefthook install

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

check-package:
    {{python}} python/scripts/check.py check-package

audit-dependencies:
    {{python}} python/scripts/check.py audit-dependencies

check-workflow:
    {{python}} python/scripts/check.py check-workflow

[positional-arguments]
release *args:
    {{python}} python/scripts/release.py "$@"

changelog:
    uvx git-cliff@2.13.1 --config python/cliff.toml --include-path "python/**" --include-path "spec/**" -o python/CHANGELOG.md
