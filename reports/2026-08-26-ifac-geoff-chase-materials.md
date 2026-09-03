# HyMeKo IFAC presentation-set revision

## Summary

Rebuilt the English presentation set around measured results. It now contains the primary A1 poster and A4 one-pager plus a detailed A1 scenario matrix and an A4 scenario/audit brief. The title is compact, the process ribbon is pictorial, and the visual system is blue-cyan-purple. Internal labels such as `strict K6` and `canonical s1` were replaced with external-facing explanations of the actual success criterion and verification scope.

The revision gives substantially more space to robot screenshots, HSiKAN ablations, real-graph AUC, learned arity behavior, NAGARE HSiKAN scaling, pruning and memory results. It also names the embodied-AI / structured-control line with Prof. Shohei Kato (NITech / Katolab).

A follow-up pass adds repeated multi-arity incidence diagrams beside the topology, HSiKAN and NAGARE plots. The NAGARE panels now combine the HSiKAN execution benchmark with noisy/few-shot quality evidence, clean and corrupted AUROC, parameter count, local-forward speed, CPU scaling, memory scaling and pruning results.

The final evidence-mining pass adds:

- a parameterized 28/44/68-link humanoid morphology result and six-view timing;
- 22-DOF Aibo standing and scripted-locomotion evidence, with learned walking honestly marked active;
- Bitcoin OTC, Bitcoin Alpha, Epinions and Slashdot results under the same-machine frozen-hyperparameter protocol;
- a separate transductive paper result whose query-edge-in-incidence scope is visibly flagged;
- strict shuffled-label leakage residual and transductive-retention metrics with dataset-level values;
- cross-dataset NAGARE/PyTorch runtime comparisons on identical exported arrays.

The layout deliberately separates experimental protocols. In particular, the transductive OTC paper AUC is not visually treated as directly comparable to the strict held-out or same-machine panels.

The presentation-integration pass then consolidates the scenario portfolio onto the primary A1 poster. Four large lower-row cards cover parameterized humanoid generation, Aibo embodied learning, same-machine signed networks, and leakage/cross-dataset checks. The former retrieval and AI-definition cards were not simply discarded: retrieval remains in the visible machine-checkable path, while the explicit AI-feature semantics move into the central HSiKAN result panel. The A4 one-pager opens with a short abstract, adds interpretation paragraphs to the FANUC, TD3, HSiKAN and NAGARE evidence blocks, and now ends with a genuine 2x2 visual scenario grid rather than a textual scenario footer. Each one-pager scenario includes a robot or result figure plus its headline metrics.

The final one-pager concept pass adds plain-language definitions of single model, hypergraph, cross-view and embodied AI. It introduces a side-by-side HSiKAN/Gömb model-family panel: HSiKAN is described as an interpretable signed-incidence spline learner, while Gömb is described as nested outer/inner representation banks and supported by the four-dataset paired comparison. The transparent HyMeKo/Fano graphic from the knowledge archive is placed behind lightly translucent cards as a restrained mandala watermark; it is an existing project asset rather than generated decoration.

Katalin Kovacs's requested research-profile pass refocuses the A4 one-pager on Csaba Hajdu. The header now includes ORCID `0000-0002-1058-7525`. An official-title PhD summary condenses *Hypergraph-Based Semantic Models in Cognitive and Robotic Systems* into the HyMeKo formalism, tensor mappings, generated domain views, structured LLM input and structural-entropy learning contribution. Six proposed AI research tracks cover verified neuro-symbolic AI, embodied/safe control, structure-aware learning, efficient local AI, cross-view digital twins and cognitive multi-agent systems. The existing scenario grid is reorganized as compact evidence for these directions, and a final collaboration block makes current interests explicit.

The one-pager now also prints `mmtdi.sze.hu/hajdu-csaba`, the official SZE public landing page that links both the dissertation and the thesis booklet. It is treated as the primary shareable dissertation reference; the direct PDF and the national doctoral record remain supporting provenance sources.

## Files touched

- `tools/build_ifac_materials.ps1` - PowerPoint/PDF generator.
- `deliverables/ifac_geoff_chase/README.md` - handoff and evidence notes.
- Generated four editable `.pptx`, four shareable `.pdf`, and four preview `.png` outputs.
- This report.

## Verification

- PowerPoint COM generated and exported all four documents.
- Generator checks every non-full-bleed object against a 12-point safe margin.
- Generator checks PowerPoint rendered text bounds; all four documents pass with zero text overflow.
- High-resolution previews were inspected for hierarchy, visible image scale and page-edge safety.
- Claims retain their experiment scope, including the retrieval shared-capture caveat, Aibo's active learned-walking status, the 40k-edge sampling note for large runtime datasets, and the transductive query-edge caveat.

## Dependencies

No project dependency added. Generation uses installed Microsoft PowerPoint 16.0 through Windows COM.

## Open issue

Adobe Express remained unavailable because the connector requested reauthentication. The local PowerPoint deliverables are complete, editable and exported to PDF. The design-skill guidance was nevertheless applied to the information hierarchy and comparison structure.

## Provenance

Content and visuals were mined from the mounted Samba share `Z:` (`\\amaterasu.local\hymeko`). No new experimental claims were generated.
