python := if os() == "windows" { "python" } else { "python3" }

default: check

setup:
    {{python}} python/scripts/check.py sync
    lefthook install

check:
    {{python}} python/scripts/check.py check
    cd rust && just check
    cd go && just check

check-integration:
    {{python}} python/scripts/check.py check-integration
    cd rust && just check-integration
    cd swift && just check-integration
    cd go && just check-integration

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
    cd swift && just check-package
    cd go && just check-package

audit-dependencies:
    {{python}} python/scripts/check.py audit-dependencies
    cd go && just audit-dependencies

check-workflow:
    {{python}} python/scripts/check.py check-workflow

[positional-arguments]
release component *args:
    @case "$1" in python|rust|swift|go) ;; *) echo "usage: just release <python|rust|swift|go> [curtains arguments]" >&2; exit 2 ;; esac
    @component="$1"; shift; \
        LEFTHOOK=0 \
        UV_TOOL_DIR="${UV_TOOL_DIR:-{{justfile_directory()}}/.cache/uv/tools}" \
        UV_CACHE_DIR="${UV_CACHE_DIR:-{{justfile_directory()}}/.cache/uv/cache}" \
        curtains --config "$component/scripts/release.json" "$@"

[positional-arguments]
release-all *args:
    #!/usr/bin/env bash
    case "$*" in "" | --dry-run) ;; *) echo "usage: just release-all [--dry-run]" >&2; exit 2 ;; esac
    release() { "{{just_executable()}}" --justfile "{{justfile()}}" release "$@"; }
    names() { if [ "$#" -gt 0 ]; then echo "$*"; else echo none; fi; }
    ready=()
    for component in python rust swift go; do
        preview=$(release "$component" --dry-run 2>&1)
        status=$?
        if [ "$status" -eq 3 ]; then
            echo "$component: nothing to release"
        elif [ "$status" -ne 0 ]; then
            printf '%s\n' "$preview" >&2
            echo "$component: preview failed; nothing was released" >&2
            exit "$status"
        elif printf '%s\n' "$preview" | grep -q "no automatic bump"; then
            echo "$component: maintenance changes only; release it with: just release $component VERSION"
        else
            printf '%s\n' "$preview" | sed '/^Dry run:/d'
            ready+=("$component")
        fi
    done
    echo
    if [ "${#ready[@]}" -eq 0 ]; then
        echo "Nothing to release."
        exit 0
    fi
    if [ "$1" = --dry-run ]; then
        echo "Dry run: would release ${ready[*]}."
        exit 0
    fi
    read -r -p "Release ${ready[*]}? [y/N] " answer < /dev/tty || answer=
    case "$answer" in y | Y | yes) ;; *) echo "Cancelled; nothing was released."; exit 130 ;; esac
    for index in "${!ready[@]}"; do
        component=${ready[$index]}
        echo "== Releasing $component"
        if ! release "$component" --yes; then
            echo "Released: $(names "${ready[@]:0:index}"). Failed: $component. Not started: $(names "${ready[@]:index+1}")." >&2
            exit 1
        fi
    done
    echo "Released ${ready[*]}."

changelog: changelog-python changelog-rust changelog-swift changelog-go

changelog-python:
    uvx git-cliff@2.13.1 --config python/cliff.toml --include-path "python/**" --include-path "spec/**" -o python/CHANGELOG.md

changelog-rust:
    uvx git-cliff@2.13.1 --config rust/cliff.toml --include-path "rust/**" --include-path "rust-toolchain.toml" -o rust/CHANGELOG.md

changelog-swift:
    uvx git-cliff@2.13.1 --config swift/cliff.toml --include-path "swift/**" --include-path "Package.swift" --include-path "Package.resolved" -o swift/CHANGELOG.md

changelog-go:
    uvx git-cliff@2.13.1 --config go/cliff.toml --include-path "go/**" -o go/CHANGELOG.md
