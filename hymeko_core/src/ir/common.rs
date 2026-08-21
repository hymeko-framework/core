use crate::common::ids::DeclId;
use crate::ir::ir::SignedRefR;

pub fn ref_target(r: &SignedRefR) -> DeclId {
    match r {
        SignedRefR::Plus(a) => a.target,
        SignedRefR::Minus(a) => a.target,
        SignedRefR::Neutral(a) => a.target,
    }
}

pub fn ref_sign(r: &SignedRefR) -> i8 {
    match r {
        SignedRefR::Plus(_) => 1,
        SignedRefR::Neutral(_) => 0,
        SignedRefR::Minus(_) => -1,
    }
}
