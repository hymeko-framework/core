//! Generational ID wire-form DESIGN + known-answer vectors (Stage 1.4).
//!
//! Design goal from the brief: Id = 8 B, Option<Id> = 8 B (niche), explicit
//! slot/generation/version fields, defined NONE / reserved / legacy decode,
//! corrupted/future rejection, no silent reinterpretation. This validates the
//! DESIGN (sizes + vectors); it is not wired into production (see the decision
//! report — no arena representation met the 1.5 lookup gate).
//!
//! Layout of the 8-byte handle (NonZeroU64 -> Option niche is free):
//!   bits [63:56]  version/scheme tag  (8 bits; 0 = this scheme)
//!   bits [55:32]  generation          (24 bits; 1..=0xFFFFFE live, 0 reserved,
//!                                       0xFFFFFF = retired marker)
//!   bits [31:0]   slot                (32 bits; up to 2^32 entities)
//! generation >= 1 is enforced, so a valid handle is always > 0 -> the value 0 is
//! never a valid handle and serves as the Option::None niche.
//!
//! Build: rustc -O wire_form.rs -o wire_form && ./wire_form

use std::num::NonZeroU64;
use std::mem::size_of;

const VER_SHIFT: u64 = 56;
const GEN_SHIFT: u64 = 32;
const SLOT_MASK: u64 = 0xFFFF_FFFF;
const GEN_MASK: u64 = 0xFF_FFFF; // 24 bits
const VER_MASK: u64 = 0xFF;
const GEN_RETIRED: u64 = 0xFF_FFFF; // saturating: reached only via retire, never reused
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
        if generation == 0 { return Err(WireErr::GenerationZero); }
        if slot > SLOT_MASK { return Err(WireErr::SlotOverflow(slot)); }
        if generation > GEN_RETIRED { return Err(WireErr::GenerationOverflow(generation)); }
        let v = (SCHEME << VER_SHIFT) | (generation << GEN_SHIFT) | slot;
        Ok(GenId(NonZeroU64::new(v).expect("generation>=1 guarantees non-zero")))
    }
    fn decode(v: NonZeroU64) -> Result<(u64, u64), WireErr> {
        let raw = v.get();
        let ver = (raw >> VER_SHIFT) & VER_MASK;
        if ver != SCHEME { return Err(WireErr::FutureVersion(ver)); }
        let generation = (raw >> GEN_SHIFT) & GEN_MASK;
        if generation == 0 { return Err(WireErr::GenerationZero); }
        let slot = raw & SLOT_MASK;
        Ok((slot, generation))
    }
    /// Legacy decode: the current serde wire is a bare index (slot only, no
    /// generation). Map it to the genesis handle (generation 1). Legacy NONE
    /// (usize::MAX) maps to Option::None, handled by the caller before this.
    fn from_legacy(index: u64) -> Result<GenId, WireErr> {
        GenId::encode(index, 1)
    }
    fn raw(self) -> u64 { self.0.get() }
}

fn main() {
    // ---- size invariants ----
    assert_eq!(size_of::<GenId>(), 8, "Id must be 8 bytes");
    assert_eq!(size_of::<Option<GenId>>(), 8, "Option<Id> must be 8 bytes (niche)");
    println!("size_of GenId={}  Option<GenId>={}  (both 8 -> OK)",
        size_of::<GenId>(), size_of::<Option<GenId>>());

    // ---- known-answer vectors ----
    let cases: &[(&str, u64, u64, u64)] = &[
        // (name, slot, generation, expected raw hex)
        ("slot 0, gen 1 (genesis)",        0,          1,          0x0000_0001_0000_0000),
        ("largest slot, gen 1",            0xFFFF_FFFF,1,          0x0000_0001_FFFF_FFFF),
        ("slot 0, largest live gen",       0,          0xFF_FFFE,  0x00FF_FFFE_0000_0000),
        ("slot 7, gen 3",                  7,          3,          0x0000_0003_0000_0007),
    ];
    println!("\n known-answer encode vectors:");
    for (name, slot, gen, exp) in cases {
        let id = GenId::encode(*slot, *gen).expect("valid");
        assert_eq!(id.raw(), *exp, "{name}");
        let (ds, dg) = GenId::decode(id.0).expect("roundtrip");
        assert_eq!((ds, dg), (*slot, *gen), "roundtrip {name}");
        println!("  {:<30} raw=0x{:016X}  decode=(slot {}, gen {})", name, id.raw(), ds, dg);
    }

    // ---- NONE ----
    let none: Option<GenId> = None;
    assert!(none.is_none());
    println!("\n NONE  = Option::None via niche (in-band value 0 is never a valid handle)");

    // ---- stale generation ----
    let live = GenId::encode(42, 5).unwrap();
    let stale = GenId::encode(42, 4).unwrap();
    assert_ne!(live, stale, "same slot, different generation -> distinct handles");
    println!(" STALE = same slot 42 gen 4 (0x{:016X}) != live gen 5 (0x{:016X}); O(1) reject on gen compare",
        stale.raw(), live.raw());

    // ---- reserved / rejection vectors ----
    println!("\n rejection vectors:");
    assert_eq!(GenId::encode(1, 0), Err(WireErr::GenerationZero));
    println!("  generation 0                 -> Err(GenerationZero)  [reserved]");
    assert_eq!(GenId::encode(1u64 << 33, 1), Err(WireErr::SlotOverflow(1u64<<33)));
    println!("  slot >= 2^32                 -> Err(SlotOverflow)");
    assert_eq!(GenId::encode(0, 0x100_0000), Err(WireErr::GenerationOverflow(0x100_0000)));
    println!("  generation > 2^24-1          -> Err(GenerationOverflow)");
    let future = NonZeroU64::new((1u64 << VER_SHIFT) | (1u64 << GEN_SHIFT) | 3).unwrap();
    assert_eq!(GenId::decode(future), Err(WireErr::FutureVersion(1)));
    println!("  version tag 1 (future scheme)-> Err(FutureVersion)   [no silent reinterpretation]");
    let genz = NonZeroU64::new(0x0000_0000_0000_0007).unwrap(); // gen field 0
    assert_eq!(GenId::decode(genz), Err(WireErr::GenerationZero));
    println!("  decode value with gen field 0-> Err(GenerationZero)  [corrupted]");

    // ---- retirement (wrap must not revive) ----
    let retired = GenId::encode(9, GEN_RETIRED).unwrap();
    println!("\n RETIRED slot 9 gen 0x{:06X} (0x{:016X}); on generation exhaustion the slot is",
        GEN_RETIRED, retired.raw());
    println!("        retired permanently (never pushed to the free list) so wrap cannot revive a stale id");

    // ---- legacy decode ----
    let leg = GenId::from_legacy(123).unwrap();
    assert_eq!(GenId::decode(leg.0).unwrap(), (123, 1));
    println!("\n LEGACY bare index 123 -> genesis handle (slot 123, gen 1) = 0x{:016X}", leg.raw());
    println!("        (old NONE = usize::MAX is mapped to Option::None before this path)");

    println!("\nALL wire-form vectors validated.");
}
