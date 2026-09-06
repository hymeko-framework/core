#!/usr/bin/env bash
set -euo pipefail
case_root=/tmp/hymeko-nagare-semantic-20260906
cd "$case_root"
mkdir evidence
exec > >(tee evidence/nagare-prepare.log) 2>&1
date -Is
while read -r repo bundle_ref revision canonical; do
  git init -q "$repo"
  git -C "$repo" remote add origin "ssh://git@amaterasu.local:2222/hymeko/$canonical.git"
  git -C "$repo" fetch -q "../$repo.bundle" "$bundle_ref"
  git -C "$repo" checkout -q --detach "$revision"
  git -C "$repo" update-ref refs/heads/main "$revision"
  printf 'SOURCE %s ' "$repo"
  git -C "$repo" rev-parse HEAD
  test -z "$(git -C "$repo" status --porcelain)"
done <<'SOURCES'
nagare refs/remotes/origin/feat/examples-learning-integration f3660cc8f738cccc69c2a011a943ce811347815e nagare
core refs/remotes/origin/audit/core-distributed-semantics-2026-09-05 125472ebfd16c9beaa96ffe2c1b00e145e2c15dd core
graph refs/remotes/origin/main 9655f07755e271c0f2ee4361a526b14e0205af8c graph
parser refs/remotes/origin/master 284c490d68232973a006bcba974279fed6f89ce8 parser
tensor refs/remotes/origin/main b1d869d24fb563ed7042f38a1272c1736fdfbea4 tensor
hymeko refs/remotes/origin/main 9d148a4dc1941761bc97a258740c1f4afe03e5b9 hymeko
SOURCES
git -C parser update-ref refs/heads/master 284c490d68232973a006bcba974279fed6f89ce8
git -C core fetch -q ../core-baseline.bundle refs/remotes/origin/fix/semantic-model-digest-v2:refs/heads/baseline-digest
git -C core diff --exit-code 125472ebfd16c9beaa96ffe2c1b00e145e2c15dd 9da5016b1fccdf89ecdcecc0862e7ae50d1adceb -- hymeko_core hymeko_wgpu
mkdir registry
git -C hymeko show 25d2d2ad8ed0a6a2508545b91f3c0d5c026669a4:roadmaps/ROADMAPS.md > registry/ROADMAPS.md
ln -s registry/ROADMAPS.md ROADMAPS.md
test -f registry/../nagare/docs/roadmaps/nagare-long-term-roadmap.md
test -f registry/../nagare/docs/roadmaps/micro-nagare-mcu-backend.md
cp nagare/Cargo.lock evidence/nagare-Cargo.lock.before
sha256sum *.bundle registry/ROADMAPS.md nagare/docs/roadmaps/*.md nagare/tests/merkle_digest_soundness.rs nagare/tests/hnn/test_helpers.rs nagare/tests/hnn_data/mlp/mlp2.hymeko hymeko/CLAUDE.md hymeko/docs/agent-contract/global.md nagare/.cargo/config.toml > evidence/nagare-input-sha256.txt
for repo in nagare core graph parser tensor hymeko; do
  git -C "$repo" status --short --branch
  git -C "$repo" log -1 --format='%H %T %s'
done > evidence/nagare-source-revisions.txt
printf 'PREPARE_EXIT=0\n'
