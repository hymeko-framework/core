<!-- SYNCED from hymeko/hymeko @ ff0c013 — docs/agent-contract/incidents/2026-07-05-fable-quarantine.md
     Do not edit here. The universal modules have ONE source; a fork of §6.5
     in each repository makes every citation in every report ambiguous.
     Edit upstream, then re-sync and record the revision. -->

---
source: CLAUDE.md preamble, FABLE / JULY-5 QUARANTINE, lines 11-12, as of ad1df25a12e60ddadb7559932f7d2b3a40a03e39
status: verbatim extraction — not yet de-projectified
---

# 2026-07-05 — the Fable / July-5 quarantine

**FABLE / JULY-5 QUARANTINE (added 2026-07-05).** Treat the July 5 Fable/Claude burst as an **untrusted import batch**, not as architecture truth. This is not a claim of proven malicious intent; it is a safety rule after observing a severe layer error. Fable confused the user's framework-level requirement ("HyMeKo itself controlled by dataflow events, FSM runtime, and monitors") with a scenario-local Galambos FSM/controller. It also introduced or propagated encoding damage (UTF-8 BOMs in modified Python files, mojibake in July 5 reports), did not start in the required Aiko register, and drifted toward blaming/redesigning the scenario when RL failed before auditing the learning implementation. Treat encoding anomalies, register/contract violations, and diagnostic-order inversions as engineering signals, not cosmetic issues. Preserve measured useful artifacts such as the push/plow Galambos controller only as scenario examples. Do **not** treat Galambos-specific FSM code as the framework architecture. Do **not** treat Gymnasium as the HyMeKo control substrate; Gymnasium is only an adapter at the physics/RL boundary. Galambos result wording is mandatory: `scripted_controller` ~0.80-0.84, `bc_clone` ~0.44-0.52, `rl_refined` worse than BC in measured runs, `framework_substrate` still to implement. Never write "RL achieved 0.52" for that result. Diagnostic order for RL failures: audit implementation first (action scaling, observations, reward timing, termination/truncation, replay distribution, critic targets, BC anchor, checkpoint/eval labeling), then teacher/BC floor, then discriminating tests; blame/redesign the scenario only after those checks fail to explain the defect. Before building on any July 5 Fable-era artifact, classify it as `keep / suspect / generated / unknown` and cite the artifact or test. Read `reports/2026-07-05-galambos-stage-ledger.md`, `reports/2026-07-05-rl-scenario-assumption-audit.md`, and `reports/2026-07-05-fable-quarantine-opus-handoff.md` before touching this area.

