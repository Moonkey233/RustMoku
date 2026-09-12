//! Candidate omission is a search policy, never a proof rule.
use rustmoku_core::{Move, Position};

use crate::{bitboard::BitBoard256, candidate_frontier::CandidateFrontier};

/// The production radius-two universe, including its empty-board center rule.
/// Construct at an offline/API boundary; normal search maintains its frontier.
pub struct ProductionCandidateUniverse {
    bits: BitBoard256,
}

impl ProductionCandidateUniverse {
    #[must_use]
    pub fn new(position: &Position) -> Self {
        Self {
            bits: if position.winner().is_some() || position.is_full() {
                BitBoard256::EMPTY
            } else {
                CandidateFrontier::new(position).candidate_bits()
            },
        }
    }

    #[must_use]
    pub fn contains(&self, at: Move) -> bool {
        self.bits.test(at)
    }

    pub fn moves(&self) -> impl Iterator<Item = Move> + '_ {
        self.bits.iter()
    }
}

/// Broad teacher root universe. Ordering/top-k output must not remove moves
/// before their scores are compared. Descendant AB search can remain selective.
pub struct TeacherCandidateUniverse;

impl TeacherCandidateUniverse {
    pub fn moves(position: &Position) -> impl Iterator<Item = Move> + '_ {
        Move::all().filter(|&at| position.is_legal(at))
    }
}

/// Complete legal universe for proof AND coverage and offline widening.
/// Deliberately has no radius, policy, top-k, or resource-based filtering option.
pub struct ProofCandidateUniverse;

impl ProofCandidateUniverse {
    pub fn moves(position: &Position) -> impl Iterator<Item = Move> + '_ {
        Move::all().filter(|&at| position.is_legal(at))
    }
}

/// Explicit root analysis policy. ProductionTopK exists for controlled ablations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TeacherCandidates {
    #[default]
    AllLegal,
    ProductionTopK,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teacher_and_proof_never_inherit_production_radius_or_center_rule() {
        let mut p = Position::default();
        assert_eq!(ProductionCandidateUniverse::new(&p).moves().count(), 1);
        assert_eq!(TeacherCandidateUniverse::moves(&p).count(), 225);
        p.make_move(Move::CENTER).unwrap();
        let far = Move::from_index(0).unwrap();
        assert!(!ProductionCandidateUniverse::new(&p).contains(far));
        let teacher: Vec<_> = TeacherCandidateUniverse::moves(&p).collect();
        let proof: Vec<_> = ProofCandidateUniverse::moves(&p).collect();
        assert_eq!(teacher, proof);
        assert_eq!(teacher.len(), 224);
        assert!(teacher.contains(&far));
    }
}
