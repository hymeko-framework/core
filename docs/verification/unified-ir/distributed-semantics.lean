import Std

/-!
Core single-writer commit-admission specification, Lean 4.19.0.

This is a normative abstraction of the admission boundary, not a verified
translation of Rust. Nat versions cover the non-arithmetic order laws of u64
versions. Identity and payload are independent opaque tags. Equal identity tags
do not prove equal payloads in this model; no digest-binding or cryptographic
collision-freedom theorem is assumed or proved.
Wire validation, sequence handling, staging, application validation, persistence,
GPU lifetime and eventual delivery remain separate proof obligations.
-/
namespace HyMeKo.Distributed

structure Version where
  topology : Nat
  weights : Nat
  features : Nat
  params : Nat
  deriving DecidableEq, Repr

def Version.dominates (a b : Version) : Prop :=
  a.topology ≥ b.topology ∧ a.weights ≥ b.weights ∧
  a.features ≥ b.features ∧ a.params ≥ b.params

instance (a b : Version) : Decidable (a.dominates b) :=
  inferInstanceAs (Decidable (_ ∧ _ ∧ _ ∧ _))

theorem dominates_refl (v : Version) : v.dominates v := by
  simp [Version.dominates]

theorem dominates_trans {a b c : Version}
    (hab : a.dominates b) (hbc : b.dominates c) : a.dominates c := by
  simp only [Version.dominates] at *
  exact ⟨Nat.le_trans hbc.1 hab.1, Nat.le_trans hbc.2.1 hab.2.1,
    Nat.le_trans hbc.2.2.1 hab.2.2.1, Nat.le_trans hbc.2.2.2 hab.2.2.2⟩

theorem dominates_antisymm {a b : Version}
    (hab : a.dominates b) (hba : b.dominates a) : a = b := by
  cases a; cases b
  simp only [Version.dominates] at *
  congr <;> omega

inductive Kind where
  | snapshot | delta
  deriving DecidableEq, Repr

structure Transaction where
  kind : Kind
  base : Version
  target : Version
  identity : Nat
  payload : Nat
  deriving DecidableEq, Repr

structure Replica where
  version : Version
  established : Bool
  identity : Option Nat
  payload : Nat
  deriving DecidableEq, Repr

def zero : Version := ⟨0, 0, 0, 0⟩
def empty : Replica := ⟨zero, false, none, 0⟩
def resume (v : Version) (payload : Nat) : Replica := ⟨v, true, none, payload⟩

def coherent (base target : Version) : Prop :=
  target.topology > base.topology →
    target.weights > base.weights ∧ target.features > base.features

instance (a b : Version) : Decidable (coherent a b) :=
  inferInstanceAs (Decidable (_ → _ ∧ _))

inductive Decision where
  | install | duplicate | superseded | incomparable | staleBase
  | incoherent | unknownIdentity | conflictingIdentity
  deriving DecidableEq, Repr

def decision (s : Replica) (t : Transaction) : Decision :=
  if s.version.dominates t.target ∧ t.target ≠ s.version then .superseded
  else if ¬t.target.dominates s.version then .incomparable
  else if t.kind = .delta ∧ t.base ≠ s.version then .staleBase
  else if ¬coherent t.base t.target then .incoherent
  else if t.target = s.version ∧ s.established = true then
    match s.identity with
    | none => .unknownIdentity
    | some identity => if identity = t.identity then .duplicate else .conflictingIdentity
  else .install

def installed (t : Transaction) : Replica := ⟨t.target, true, some t.identity, t.payload⟩

def apply (s : Replica) (t : Transaction) : Replica :=
  if decision s t = .install then installed t else s

theorem rejected_preserves (s : Replica) (t : Transaction)
    (h : decision s t ≠ .install) : apply s t = s := by
  simp [apply, h]

theorem admitted_dominates (s : Replica) (t : Transaction)
    (h : decision s t = .install) : t.target.dominates s.version := by
  unfold decision at h
  split at h
  · contradiction
  · split at h
    · contradiction
    · simp_all

theorem apply_monotone (s : Replica) (t : Transaction) :
    (apply s t).version.dominates s.version := by
  by_cases h : decision s t = .install
  · simpa [apply, h, installed] using admitted_dominates s t h
  · simpa [apply, h] using dominates_refl s.version

theorem equal_established_not_installed (s : Replica) (t : Transaction)
    (hv : t.target = s.version) (he : s.established = true) :
    decision s t ≠ .install := by
  by_cases hd : t.kind = .delta ∧ t.base ≠ s.version
  · simp [decision, hv, he, dominates_refl, hd]
  · by_cases hc : coherent t.base s.version
    · cases hi : s.identity with
      | none => simp [decision, hv, he, dominates_refl, hd, hc, hi]
      | some identity =>
        simp [decision, hv, he, dominates_refl, hd, hc, hi]
        split <;> simp
    · simp [decision, hv, he, dominates_refl, hd, hc]

theorem same_version_preserves (s : Replica) (t : Transaction)
    (hv : t.target = s.version) (he : s.established = true) : apply s t = s :=
  rejected_preserves s t (equal_established_not_installed s t hv he)

theorem apply_idempotent (s : Replica) (t : Transaction) :
    apply (apply s t) t = apply s t := by
  by_cases h : decision s t = .install
  · simp only [apply, h, ↓reduceIte]
    have hn := equal_established_not_installed (installed t) t rfl rfl
    simp [hn]
  · simp [apply, h]

theorem stale_delta_not_installed (s : Replica) (t : Transaction)
    (hk : t.kind = .delta) (hb : t.base ≠ s.version) : decision s t ≠ .install := by
  unfold decision
  split <;> simp_all
  split <;> simp_all

theorem incomparable_not_installed (s : Replica) (t : Transaction)
    (ht : ¬t.target.dominates s.version) : decision s t ≠ .install := by
  unfold decision
  split <;> simp_all

def replay (s : Replica) (transactions : List Transaction) : Replica :=
  transactions.foldl apply s

theorem replay_monotone (transactions : List Transaction) (s : Replica) :
    (replay s transactions).version.dominates s.version := by
  induction transactions generalizing s with
  | nil => exact dominates_refl s.version
  | cons t ts ih =>
    exact dominates_trans (ih (apply s t)) (apply_monotone s t)

theorem adjacent_replay_is_idempotent (s : Replica) (t : Transaction)
    (rest : List Transaction) : replay s (t :: t :: rest) = replay s (t :: rest) := by
  simp [replay, List.foldl, apply_idempotent]

theorem newer_snapshot_installed (s : Replica) (t : Transaction)
    (hk : t.kind = .snapshot) (hd : t.target.dominates s.version)
    (hne : t.target ≠ s.version) (hc : coherent t.base t.target) :
    apply s t = installed t := by
  have hn : ¬s.version.dominates t.target := by
    intro h
    exact hne (dominates_antisymm hd h)
  simp [apply, decision, hn, hd, hk, hc, hne]

theorem replicas_converge_on_newer_snapshot (a b : Replica) (t : Transaction)
    (hk : t.kind = .snapshot) (ha : t.target.dominates a.version)
    (hb : t.target.dominates b.version) (hna : t.target ≠ a.version)
    (hnb : t.target ≠ b.version) (hc : coherent t.base t.target) :
    apply a t = apply b t := by
  rw [newer_snapshot_installed a t hk ha hna hc,
      newer_snapshot_installed b t hk hb hnb hc]

/- Closed admission examples correspond to the independently run Rust probes. -/
def v : Version := ⟨2, 2, 2, 0⟩
def sameVersion : Transaction := ⟨.snapshot, zero, v, 99, 7⟩
def incomparable : Transaction := ⟨.snapshot, zero, ⟨1, 3, 3, 0⟩, 99, 7⟩
def zeroSnapshot : Transaction := ⟨.snapshot, zero, zero, 99, 7⟩

theorem resumed_identity_unknown : decision (resume v 5) sameVersion = .unknownIdentity := by decide
theorem incomparable_snapshot_rejected : decision (resume v 5) incomparable = .incomparable := by decide
theorem fresh_zero_bootstrap : decision empty zeroSnapshot = .install := by decide
theorem restored_zero_not_bootstrap : decision (resume zero 5) zeroSnapshot = .unknownIdentity := by decide

#print axioms dominates_refl
#print axioms dominates_trans
#print axioms dominates_antisymm
#print axioms rejected_preserves
#print axioms admitted_dominates
#print axioms apply_monotone
#print axioms equal_established_not_installed
#print axioms same_version_preserves
#print axioms apply_idempotent
#print axioms stale_delta_not_installed
#print axioms incomparable_not_installed
#print axioms replay_monotone
#print axioms adjacent_replay_is_idempotent
#print axioms newer_snapshot_installed
#print axioms replicas_converge_on_newer_snapshot
#print axioms resumed_identity_unknown
#print axioms incomparable_snapshot_rejected
#print axioms fresh_zero_bootstrap
#print axioms restored_zero_not_bootstrap

end HyMeKo.Distributed
