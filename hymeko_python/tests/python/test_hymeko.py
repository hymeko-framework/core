"""Stdlib-unittest suite for the `hymeko` Python bindings (mission §8).

Runs against the *installed* module (wheel or `maturin develop`), driving the real
parser → resolve → lower → validate → canonical-IR pipeline. No test builder.
"""

from __future__ import annotations

import unittest

import hymeko

import _support as sup


class TestImportAndParse(unittest.TestCase):
    def test_module_imports_and_has_surface(self):
        for name in [
            "parse", "load", "Model", "DeclId", "NodeId", "EdgeId", "HyperArcId",
            "HyMeKoError", "HyMeKoSyntaxError", "HyMeKoResolutionError",
            "HyMeKoLoweringError", "HyMeKoValidationError", "HyMeKoIoError",
            "HyMeKoIdError",
        ]:
            self.assertTrue(hasattr(hymeko, name), f"missing {name}")

    def test_in_memory_parse(self):
        m = hymeko.parse(sup.MIXED_SOURCE, filename="<memory>")
        self.assertGreaterEqual(m.node_count, 3)
        self.assertEqual(m.edge_count, 1)
        self.assertEqual(m.hyperarc_count, 2)

    def test_file_parse(self):
        m = hymeko.load(sup.fixture("minimal_examples/minimal_example_base_elements.hymeko"))
        self.assertEqual(m.node_count, 2)
        self.assertEqual(m.hyperarc_count, 1)

    def test_import_fixture(self):
        # basic_import_example imports minimal_example_library.hymeko.
        m = hymeko.load(sup.fixture("minimal_examples/import_examples/minimal_example_import.hymeko"))
        self.assertGreater(m.declaration_count, 0)
        # a canonical hash is computable over the merged program
        self.assertEqual(len(m.canonical_hash()), 64)


class TestErrors(unittest.TestCase):
    def test_syntax_error(self):
        with self.assertRaises(hymeko.HyMeKoSyntaxError):
            hymeko.parse("MixedDesc{}\n@@@ not valid")

    def test_resolution_or_lowering_error_on_undefined_ref(self):
        # parses, but references an undeclared node -> a typed HyMeKo error
        # (resolution or lowering, both subclasses of HyMeKoError), never a panic.
        with self.assertRaises(hymeko.HyMeKoError):
            hymeko.parse("D{}\nA;\n@E { (+A, -Undefined); }\n")

    def test_io_error_on_missing_file(self):
        with self.assertRaises(hymeko.HyMeKoIoError):
            hymeko.load("does/not/exist.hymeko")


class TestCountsAndHash(unittest.TestCase):
    def test_entity_counts(self):
        m = hymeko.load(sup.fixture("typical_graphs/k4_3uniform.hymeko"))
        self.assertEqual(m.declaration_count, len(m.declarations()))
        self.assertEqual(m.node_count, len(m.nodes()))
        self.assertEqual(m.edge_count, len(m.edges()))
        self.assertEqual(m.hyperarc_count, len(m.hyperarcs()))

    def test_canonical_hash_parity_with_rust_oracle(self):
        for rel, expected in sup.ORACLE.items():
            with self.subTest(fixture=rel):
                m = hymeko.load(sup.fixture(rel))
                self.assertEqual(m.canonical_hash(), expected)

    def test_hash_is_deterministic(self):
        a = hymeko.parse(sup.MIXED_SOURCE)
        b = hymeko.parse(sup.MIXED_SOURCE)
        self.assertEqual(a.canonical_hash(), b.canonical_hash())


class TestHierarchyAndEntities(unittest.TestCase):
    def test_hierarchy_parent_children(self):
        m = hymeko.load(sup.fixture("minimal_examples/minimal_example_basic_hierarchy.hymeko"))
        # every child's parent_of points back; children_of is consistent
        for d in m.declarations():
            for c in m.children_of(d.id):
                self.assertEqual(m.parent_of(c), d.id)

    def test_node_edge_hyperarc_read(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        n0 = m.nodes()[0]
        self.assertEqual(n0.name, "A")
        self.assertIsNotNone(n0.decl)
        e0 = m.edges()[0]
        self.assertEqual(len(e0.hyperarcs), 2)
        h0 = m.hyperarcs()[0]
        self.assertEqual(h0.arity, 3)

    def test_signs_plus_minus_neutral(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        signs = [inc.sign for inc in m.hyperarcs()[0].incidences]
        self.assertEqual(signs, [1, -1, 0])  # +A, -B, ~C

    def test_weights(self):
        m = hymeko.load(sup.fixture("minimal_examples/minimal_example_fields_with_reference.hymeko"))
        # a declaration with a typed value carries an annotation value
        vals = [d.annotation["value"] for d in m.declarations()]
        self.assertTrue(any(v is not None for v in vals))

    def test_annotations_and_values(self):
        # this fixture carries ref weight-values on its incidences (`node0[0.85]`).
        m = hymeko.load(sup.fixture("minimal_examples/tag_annotations/minimal_tag_annotation.hymeko"))
        weighted = [
            inc.weights
            for h in m.hyperarcs()
            for inc in h.incidences
            if inc.weights is not None
        ]
        self.assertTrue(weighted)
        self.assertIn([0.85], weighted)

    def test_isa_base_links(self):
        # node10: + <isa> node0  -> node10's bases include node0.
        m = hymeko.load(sup.fixture("minimal_examples/tag_annotations/minimal_multi_tag_annotation.hymeko"))
        by_name = {n.name: n for n in m.nodes()}
        self.assertIn("node10", by_name)
        base_targets = [b.target for b in by_name["node10"].bases]
        self.assertEqual(by_name["node0"].decl, base_targets[0])
        self.assertTrue(all(b.sign in (-1, 0, 1) for b in by_name["node10"].bases))

    def test_duplicate_incidence_kept(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        arc1 = m.hyperarcs()[1]  # (+A, +A)
        targets = [inc.target for inc in arc1.incidences]
        self.assertEqual(len(targets), 2)
        self.assertEqual(targets[0], targets[1])  # same DeclId, kept as two coords

    def test_canonical_path(self):
        m = hymeko.load(sup.fixture("minimal_examples/minimal_example_basic_hierarchy.hymeko"))
        paths = [d.canonical_path for d in m.declarations() if d.canonical_path]
        self.assertTrue(any("." in p for p in paths))  # a nested path exists


class TestEmptyAndUnicode(unittest.TestCase):
    def test_empty_graph(self):
        m = hymeko.parse("Empty{}\n")
        self.assertEqual(m.node_count, 0)
        self.assertEqual(m.edge_count, 0)
        self.assertEqual(m.hyperarc_count, 0)
        self.assertEqual(len(m.incidence_arrays().targets), 0)
        self.assertEqual(len(m.canonical_hash()), 64)

    def test_unicode_string_value_roundtrips(self):
        src = 'U{}\nctx {\n  v <string> "vakråb-é中";\n}\n'
        m = hymeko.parse(src)
        found = [d.annotation["value"] for d in m.declarations()]
        self.assertIn("vakråb-é中", found)


class TestIdSafety(unittest.TestCase):
    def test_cross_model_id_rejected(self):
        m1 = hymeko.parse(sup.MIXED_SOURCE)
        m2 = hymeko.parse(sup.MIXED_SOURCE)
        nid = m1.nodes()[0].id
        with self.assertRaises(hymeko.HyMeKoIdError):
            m2.node(nid)

    def test_wrong_domain_id_rejected(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        nid = m.nodes()[0].id
        # passing a NodeId where a DeclId is expected is a TypeError at the boundary
        with self.assertRaises(TypeError):
            m.declaration(nid)

    def test_ids_not_interchangeable_types(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        self.assertIsInstance(m.nodes()[0].id, hymeko.NodeId)
        self.assertNotIsInstance(m.nodes()[0].id, hymeko.DeclId)

    def test_id_equality_and_hash(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        a = m.nodes()[0].id
        b = m.nodes()[0].id
        self.assertEqual(a, b)
        self.assertEqual(hash(a), hash(b))
        self.assertEqual(len({a, b}), 1)  # usable as dict/set keys


class TestDeterminismAndLifetime(unittest.TestCase):
    def test_deterministic_iteration(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        first = [n.name for n in m.nodes()]
        second = [n.name for n in m.nodes()]
        self.assertEqual(first, second)

    def test_model_lifetime_via_entities(self):
        # the Model expression result is not bound; entities must keep it alive.
        nodes = hymeko.parse(sup.MIXED_SOURCE).nodes()
        self.assertEqual(nodes[0].name, "A")
        inc = hymeko.parse(sup.MIXED_SOURCE).hyperarcs()[0].incidences
        self.assertEqual(inc[0].sign, 1)


class TestBatchArrays(unittest.TestCase):
    def test_batch_matches_scalar_path(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        ia = m.incidence_arrays()
        # arrays are typed memoryviews (a bulk buffer, not a list of Python ints)
        self.assertIsInstance(ia.targets, memoryview)
        offs = list(ia.arc_offsets)
        targets = list(ia.targets)
        signs = list(ia.signs)
        slots = list(ia.slots)
        # reconstruct per-arc incidences from the SoA and compare to the scalar API
        for ai, h in enumerate(m.hyperarcs()):
            lo, hi = offs[ai], offs[ai + 1]
            scalar = [(inc.slot, inc.sign, inc.target) for inc in h.incidences]
            self.assertEqual(hi - lo, len(scalar))
            for k, (slot, sign, target) in zip(range(lo, hi), scalar):
                self.assertEqual(slots[k], slot)
                self.assertEqual(signs[k], sign)
                target_raw = int(repr(target).removeprefix("DeclId(").removesuffix(")"))
                self.assertEqual(targets[k], target_raw)
        # weights present and finite
        self.assertEqual(len(list(ia.weight_scalars)), ia.nnz)

    def test_batch_target_raw_matches(self):
        m = hymeko.parse(sup.MIXED_SOURCE)
        ia = m.incidence_arrays()
        targets = list(ia.targets)
        # the arc-major flatten of the scalar incidences' target raw must match
        flat = []
        for h in m.hyperarcs():
            for inc in h.incidences:
                # inc.target is a DeclId; its raw isn't public, but repr encodes it
                flat.append(int(repr(inc.target).removeprefix("DeclId(").removesuffix(")")))
        self.assertEqual(targets, flat)


class TestGolden(unittest.TestCase):
    def test_golden_typical_graphs(self):
        for rel in [
            "typical_graphs/fano_graph.hymeko",
            "typical_graphs/sunflower_delta_system.hymeko",
            "typical_graphs/k4_3uniform.hymeko",
            "typical_graphs/generic_hypergraph.hymeko",
        ]:
            with self.subTest(fixture=rel):
                m = hymeko.load(sup.fixture(rel))
                self.assertEqual(m.canonical_hash(), sup.ORACLE[rel])


class TestScale(unittest.TestCase):
    def _check_windowed(self, n_nodes, arity):
        src = sup.windowed_source("Scale", n_nodes, arity)
        m = hymeko.parse(src, filename="<scale>")
        self.assertEqual(m.node_count, n_nodes)
        exp = sup.windowed_incidence_count(n_nodes, arity)
        ia = m.incidence_arrays()
        self.assertEqual(ia.nnz, exp)
        self.assertEqual(len(list(ia.targets)), exp)
        self.assertEqual(list(ia.arc_offsets)[-1], exp)
        # determinism: reparsing yields the same hash
        self.assertEqual(m.canonical_hash(), hymeko.parse(src).canonical_hash())

    def test_1e3_incidences(self):
        self._check_windowed(1004, 5)  # 1000 incidences

    def test_1e4_incidences(self):
        self._check_windowed(10004, 5)  # 10000 incidences

    def test_1e5_incidences(self):
        self._check_windowed(100004, 5)  # 100000 incidences


if __name__ == "__main__":
    unittest.main(verbosity=2)
