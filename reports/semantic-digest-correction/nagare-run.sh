#!/usr/bin/env bash
set -euo pipefail
cd /workspace/nagare
exec > >(tee /workspace/evidence/nagare-test.log) 2>&1
trap 'result=$?; cat /sys/fs/cgroup/memory.peak > /workspace/evidence/nagare-memory-peak.txt; printf "%s\n" "$result" > /workspace/evidence/nagare-exit.txt; date -Is; exit "$result"' EXIT
date -Is
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export CARGO_NET_OFFLINE=false
export GIT_CONFIG_COUNT=13
export GIT_CONFIG_KEY_0=url.file:///workspace/core.insteadOf
export GIT_CONFIG_VALUE_0=ssh://git@amaterasu.local:2222/hymeko/core.git
export GIT_CONFIG_KEY_1=url.file:///workspace/graph.insteadOf
export GIT_CONFIG_VALUE_1=ssh://git@amaterasu.local:2222/hymeko/graph.git
export GIT_CONFIG_KEY_2=url.file:///workspace/parser.insteadOf
export GIT_CONFIG_VALUE_2=ssh://git@amaterasu.local:2222/hymeko/parser.git
export GIT_CONFIG_KEY_3=protocol.file.allow
export GIT_CONFIG_VALUE_3=always
export GIT_CONFIG_KEY_4=safe.directory
export GIT_CONFIG_VALUE_4=/workspace/nagare
export GIT_CONFIG_KEY_5=safe.directory
export GIT_CONFIG_VALUE_5=/workspace/core
export GIT_CONFIG_KEY_6=safe.directory
export GIT_CONFIG_VALUE_6=/workspace/graph
export GIT_CONFIG_KEY_7=safe.directory
export GIT_CONFIG_VALUE_7=/workspace/parser
export GIT_CONFIG_KEY_8=safe.directory
export GIT_CONFIG_VALUE_8=/workspace/tensor
export GIT_CONFIG_KEY_9=safe.directory
export GIT_CONFIG_VALUE_9=/workspace/hymeko
export GIT_CONFIG_KEY_10=safe.directory
export GIT_CONFIG_VALUE_10=/workspace/core/.git
export GIT_CONFIG_KEY_11=safe.directory
export GIT_CONFIG_VALUE_11=/workspace/graph/.git
export GIT_CONFIG_KEY_12=safe.directory
export GIT_CONFIG_VALUE_12=/workspace/parser/.git
# upload-pack removes Git command-scoped environment for another repository.
# Its ownership check therefore needs exact paths in this disposable container's
# own global config; no workstation, source-repository or shared-cache config changes.
for mirror in core graph parser; do
  git config --global --add safe.directory "/workspace/$mirror/.git"
  git ls-remote "file:///workspace/$mirror" HEAD
done
printf 'CONTROL\n'
command -v cargo rustc git
cargo --version
rustc -Vv
test -x /usr/bin/time
test "$(cat /sys/fs/cgroup/memory.max)" = 17179869184
test "$(git -C ../hymeko rev-parse HEAD)" = 9d148a4dc1941761bc97a258740c1f4afe03e5b9
test -z "$(git -C ../hymeko status --porcelain)"
test -f ../ROADMAPS.md
test -f docs/roadmaps/nagare-long-term-roadmap.md
test -f docs/roadmaps/micro-nagare-mcu-backend.md
(cd /workspace && sha256sum -c evidence/nagare-input-sha256.txt)
printf 'BASELINE_METADATA\n'
/usr/bin/time -f 'resource stage=metadata_before elapsed_s=%e max_rss_kib=%M exit=%x' cargo metadata --locked --format-version 1 --filter-platform x86_64-unknown-linux-gnu > ../evidence/nagare-metadata-before.json
cargo tree --locked --target x86_64-unknown-linux-gnu -i hymeko_core > ../evidence/nagare-core-tree-before.txt
cargo tree --locked --target x86_64-unknown-linux-gnu -i hymeko_wgpu > ../evidence/nagare-wgpu-tree-before.txt
cargo tree --locked --target x86_64-unknown-linux-gnu -i hymeko_graph > ../evidence/nagare-graph-tree-before.txt
printf 'SCRATCH_LOCK_UPDATE\n'
/usr/bin/time -f 'resource stage=lock_update elapsed_s=%e max_rss_kib=%M exit=%x' cargo update -p hymeko_core --precise 125472ebfd16c9beaa96ffe2c1b00e145e2c15dd
cp Cargo.lock ../evidence/nagare-Cargo.lock.after
git diff -- Cargo.lock > ../evidence/nagare-lock.diff
cargo metadata --locked --format-version 1 --filter-platform x86_64-unknown-linux-gnu > ../evidence/nagare-metadata-after.json
cargo tree --locked --target x86_64-unknown-linux-gnu -i hymeko_core > ../evidence/nagare-core-tree-after.txt
cargo tree --locked --target x86_64-unknown-linux-gnu -i hymeko_wgpu > ../evidence/nagare-wgpu-tree-after.txt
cargo tree --locked --target x86_64-unknown-linux-gnu -i hymeko_graph > ../evidence/nagare-graph-tree-after.txt
git status --short > ../evidence/nagare-status-before-test.txt
sha256sum Cargo.lock > ../evidence/nagare-lock-before-test.sha256
printf 'TEST_14\n'
/usr/bin/time -f 'resource stage=test elapsed_s=%e max_rss_kib=%M exit=%x' cargo test --release --locked --offline -p hymeko_nagare --test merkle_digest_soundness -- --nocapture
sha256sum -c ../evidence/nagare-lock-before-test.sha256
git status --short > ../evidence/nagare-status-after-test.txt
git diff --exit-code -- . ':!Cargo.lock'
printf 'NAGARE_GATE_EXIT=0\n'
