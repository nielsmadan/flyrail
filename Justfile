python := if os() == "windows" { "python" } else { "python3" }

default: check

setup:
    {{python}} python/scripts/check.py sync
    lefthook install

check:
    {{python}} python/scripts/check.py check
    cd rust && just check

check-integration:
    {{python}} python/scripts/check.py check-integration
    cd rust && just check-integration

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
    cd rust && just check-package

audit-dependencies:
    {{python}} python/scripts/check.py audit-dependencies

check-workflow:
    {{python}} python/scripts/check.py check-workflow

[positional-arguments]
release *args:
    @UV_TOOL_DIR="${UV_TOOL_DIR:-{{justfile_directory()}}/.cache/uv/tools}" \
        UV_CACHE_DIR="${UV_CACHE_DIR:-{{justfile_directory()}}/.cache/uv/cache}" \
        curtains --config python/scripts/release.json "$@"

changelog:
    uvx git-cliff@2.13.1 --config python/cliff.toml --include-path "python/**" --include-path "spec/**" -o python/CHANGELOG.md
