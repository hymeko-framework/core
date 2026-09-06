//! Frozen contracts for the generational-arena phase (Stage 1.1/1.4).
//!
//! The current ID contracts (size, NONE, bare-integer serde wire, sealed
//! construction) are already frozen in `test_ids.rs`; this file freezes the
//! *target* generational wire form so a future migration has a known-answer
//! gate. The design is validated here but is **not** wired into production —
//! see `reports/2026-08-31-generational-arena-decision.md`: no arena
//! representation met the CLAUDE.md 1.5 valid-lookup gate (best case +56 %),
//! so the migration is deferred.
//!
//! When/if the migration happens, these vectors are the contract it must meet:
//! `GenId` is 8 B, `Option<GenId>` is 8 B (niche), and encode/decode obeys the
//! fixed layout below. Divergence must consciously update this file.

use std::mem::size_of;
use std::num::NonZeroU64;

// Layout of the 8-byte handle:
//   [63:56] version/scheme tag (0 = this scheme)
//   [55:32] generation (24 bits; 1..=0xFFFFFE live, 0 reserved, 0xFFFFFF retired)
//   [31:0]  slot (32 bits)
// generation >= 1 keeps the value non-zero, so 0 is the Option::None niche.
const VER_SHIFT: u64 = 56;
const GEN_SHIFT: u64 = 32;
const SLOT_MASK: u64 = 0xFFFF_FFFF;
const GEN_MASK: u64 = 0xFF_FFFF;
const VER_MASK: u64 = 0xFF;
const GEN_RETIRED: u64 = 0xFF_FFFF;
const SCHEME: u64 = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GenId(NonZeroU64);

#[derive(Debug, PartialEq, Eq)]
enum WireErr {
    GenerationZero,
    FutureVersion(u64),
    SlotOverflow(u64),
    GenerationOverflow(u64),
}

impl GenId {
    fn encode(slot: u64, generation: u64) -> Result<GenId, WireErr> {
        if generation == 0 {
            return Err(WireErr::GenerationZero);
        }
        if slot > SLOT_MASK {
            return Err(WireErr::SlotOverflow(slot));
        }
        if generation > GEN_RETIRED {
            return Err(WireErr::GenerationOverflow(generation));
        }
        let v = (SCHEME << VER_SHIFT) | (generation << GEN_SHIFT) | slot;
        Ok(GenId(
            NonZeroU64::new(v).expect("generation>=1 => non-zero"),
        ))
    }
    fn decode(v: NonZeroU64) -> Result<(u64, u64), WireErr> {
        let raw = v.get();
        let ver = (raw >> VER_SHIFT) & VER_MASK;
        if ver != SCHEME {
            return Err(WireErr::FutureVersion(ver));
        }
        let generation = (raw >> GEN_SHIFT) & GEN_MASK;
        if generation == 0 {
            return Err(WireErr::GenerationZero);
        }
        Ok((raw & SLOT_MASK, generation))
    }
    fn from_legacy(index: u64) -> Result<GenId, WireErr> {
        GenId::encode(index, 1)
    }
    fn raw(self) -> u64 {
        self.0.get()
    }
}

#[test]
fn genid_size_targets_are_eight_bytes() {
    assert_eq!(size_of::<GenId>(), 8, "target: Id is 8 bytes");
    assert_eq!(
        size_of::<Option<GenId>>(),
        8,
        "target: Option<Id> is 8 bytes via the niche"
    );
}

#[test]
fn genid_known_answer_encode_vectors() {
    let cases: &[(&str, u64, u64, u64)] = &[
        ("genesis slot0 gen1", 0, 1, 0x0000_0001_0000_0000),
        ("largest slot gen1", 0xFFFF_FFFF, 1, 0x0000_0001_FFFF_FFFF),
        (
            "slot0 largest live gen",
            0,
            0xFF_FFFE,
            0x00FF_FFFE_0000_0000,
        ),
        ("slot7 gen3", 7, 3, 0x0000_0003_0000_0007),
    ];
    for (name, slot, generation, expected) in cases {
        let id = GenId::encode(*slot, *generation).expect(name);
        assert_eq!(id.raw(), *expected, "encode {name}");
        assert_eq!(
            GenId::decode(id.0).unwrap(),
            (*slot, *generation),
            "decode {name}"
        );
    }
}

#[test]
fn genid_stale_generation_is_a_distinct_handle() {
    let live = GenId::encode(42, 5).unwrap();
    let stale = GenId::encode(42, 4).unwrap();
    assert_ne!(live, stale, "same slot, different generation => distinct");
}

#[test]
fn genid_rejection_vectors_never_reinterpret_silently() {
    assert_eq!(GenId::encode(1, 0), Err(WireErr::GenerationZero));
    assert_eq!(
        GenId::encode(1u64 << 33, 1),
        Err(WireErr::SlotOverflow(1u64 << 33))
    );
    assert_eq!(
        GenId::encode(0, 0x100_0000),
        Err(WireErr::GenerationOverflow(0x100_0000))
    );
    let future = NonZeroU64::new((1u64 << VER_SHIFT) | (1u64 << GEN_SHIFT) | 3).unwrap();
    assert_eq!(GenId::decode(future), Err(WireErr::FutureVersion(1)));
    let gen_field_zero = NonZeroU64::new(0x0000_0000_0000_0007).unwrap();
    assert_eq!(GenId::decode(gen_field_zero), Err(WireErr::GenerationZero));
}

#[test]
fn genid_retirement_marker_is_the_saturated_generation() {
    let retired = GenId::encode(9, GEN_RETIRED).unwrap();
    assert_eq!(retired.raw(), 0x00FF_FFFF_0000_0009);
    // On generation exhaustion the slot is retired (never reused), so wrap
    // cannot revive a stale id.
}

#[test]
fn genid_legacy_bare_index_maps_to_the_genesis_handle() {
    let leg = GenId::from_legacy(123).unwrap();
    assert_eq!(GenId::decode(leg.0).unwrap(), (123, 1));
}
