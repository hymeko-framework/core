#!/bin/sh
# One container/CI entry point composing the existing Core verification paths.
set -eu

action=${1:-all}
if [ "$#" -gt 1 ]; then
    printf 'Expected at most one verification action.\n' >&2
    exit 2
fi
case "$action" in
    smoke|test|lint|bench-smoke|all) ;;
    *) printf 'usage: hymeko-core-check [smoke|test|lint|bench-smoke|all]\n' >&2; exit 2 ;;
esac

# A bounded container is part of the execution contract, not a host convention.
memory_limit=$(cat /sys/fs/cgroup/memory.max)
case "$memory_limit" in
    ''|*[!0-9]*) printf 'Run with --memory=16g --memory-swap=16g (or less).\n' >&2; exit 2 ;;
esac
if [ "$memory_limit" -gt 17179869184 ]; then
    printf 'Container memory limit exceeds 16 GiB.\n' >&2
    exit 2
fi
if [ "$(cat /sys/fs/cgroup/memory.swap.max)" != 0 ]; then
    printf 'Disable container swap with equal --memory and --memory-swap.\n' >&2
    exit 2
fi

run() {
    stage=$1
    shift
    printf '\n[%s] stage=%s\n' "$(date -u +%FT%TZ)" "$stage"
    /usr/bin/time -f "resource stage=$stage elapsed_s=%e max_rss_kib=%M exit=%x" "$@" &
    command_pid=$!
    (
        while kill -0 "$command_pid" 2>/dev/null; do
            sleep 30
            if kill -0 "$command_pid" 2>/dev/null; then
                printf '[%s] stage=%s cgroup_memory_bytes=%s\n' \
                    "$(date -u +%FT%TZ)" "$stage" "$(cat /sys/fs/cgroup/memory.current)"
            fi
        done
    ) &
    monitor_pid=$!
    result=0
    wait "$command_pid" || result=$?
    kill "$monitor_pid" 2>/dev/null || true
    wait "$monitor_pid" 2>/dev/null || true
    printf 'resource stage=%s cgroup_container_peak_bytes=%s\n' "$stage" "$(cat /sys/fs/cgroup/memory.peak)"
    return "$result"
}

case "$action" in
    smoke)
        run cli cargo run -p hymeko_core --bin hymeko_core --release --locked --offline \
            --no-default-features -- data/minimal_examples/minimal_example.hymeko
        run cbor-corpus cargo test -p hymeko_core --release --locked --offline --test mod \
            unified_ir::validation::cbor_roundtrip::every_lowerable_corpus_program_round_trips_through_the_public_facade -- --exact
        ;;
    test)
        run core-tests cargo test -p hymeko_core --release --locked --offline \
            --no-default-features --no-fail-fast
        ;;
    lint)
        run format cargo fmt --all -- --check
        run clippy cargo clippy --workspace --all-targets --locked --offline -- -D warnings
        ;;
    bench-smoke)
        run cbor-bench-smoke cargo bench -p hymeko_core --locked --offline --bench cbor_bench -- --test
        ;;
    all)
        rustc --version --verbose
        cargo --version
        run dependencies cargo tree --workspace --locked --offline -d
        for check in smoke test lint bench-smoke; do "$0" "$check"; done
        ;;
esac
