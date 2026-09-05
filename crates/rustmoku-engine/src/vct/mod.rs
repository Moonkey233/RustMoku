//! Bounded exact Freestyle threat proofs. An unsuccessful bounded proof is not
//! a game-theoretic loss. Board-only make/unmake keeps evaluators uninvolved.
mod dfpn;
mod table;
mod threat;

pub(crate) use dfpn::VctSolver;
pub(crate) use threat::attacks;

use crate::{
    bitboard::BitBoard256,
    board_state::BoardState,
    principal_variation::PvTable,
    tactical::{ImmediateTactic, immediate_tactic},
};
use rustmoku_core::{Move, Stone};
use table::Numbers;
use threat::ThreatDescriptor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VctStatus {
    ProvenWin { plies: u8 },
    NoProof,
    BudgetExceeded,
    Interrupted,
}

pub(crate) struct VctResult {
    pub(crate) status: VctStatus,
    pub(crate) principal_variation: Vec<Move>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VctStatistics {
    pub(crate) nodes: u64,
    pub(crate) cache_hits: u64,
    pub(crate) proven: u64,
    pub(crate) budget_exhausted: u64,
}

#[derive(Clone, Copy)]
struct Fact {
    distance: Option<u8>,
    at: Option<Move>,
    reply: Option<Move>,
}

impl Fact {
    const NO_PROOF: Self = Self {
        distance: None,
        at: None,
        reply: None,
    };

    fn numbers(self) -> Numbers {
        if self.distance.is_some() {
            Numbers::WIN
        } else {
            Numbers::NO_PROOF
        }
    }

    fn write_pv(self, ply: u8, pv: &mut PvTable) {
        pv.clear(ply);
        if let Some(at) = self.at {
            pv.clear(ply + 1);
            if let Some(reply) = self.reply {
                pv.clear(ply + 2);
                pv.update(ply + 1, reply);
            }
            pv.update(ply, at);
        }
    }
}

fn fact(board: &BoardState, attacker: Stone, depth: u8) -> Option<Fact> {
    if let Some(winner) = board.position().winner() {
        return Some(Fact {
            distance: (winner == attacker).then_some(0),
            at: None,
            reply: None,
        });
    }
    if board.position().is_full() {
        return Some(Fact::NO_PROOF);
    }
    let side = board.position().side_to_move();
    let resolved = match immediate_tactic(board.patterns(), side) {
        ImmediateTactic::Win(at) if side == attacker => Some(Fact {
            distance: Some(1),
            at: Some(at),
            reply: None,
        }),
        ImmediateTactic::Win(_) => Some(Fact::NO_PROOF),
        ImmediateTactic::Loss { at, reply } if side != attacker => Some(Fact {
            distance: Some(2),
            at: Some(at),
            reply: Some(reply),
        }),
        ImmediateTactic::Loss { .. } => Some(Fact::NO_PROOF),
        ImmediateTactic::None | ImmediateTactic::ForcedBlock(_) => None,
    };
    if let Some(resolved) = resolved {
        return Some(if resolved.distance.is_some_and(|d| d > depth) {
            Fact::NO_PROOF
        } else {
            resolved
        });
    }
    // With immediate wins/double points already resolved, a nonterminal
    // attack needs at least three plies; a defender turn needs at least four
    // (its unique block, if any, removes the last current winning point).
    (depth < if side == attacker { 3 } else { 4 }).then_some(Fact::NO_PROOF)
}

fn branches(board: &BoardState, attacker: Stone, active: Option<ThreatDescriptor>) -> BitBoard256 {
    let side = board.position().side_to_move();
    if let Some(at) = immediate_tactic(board.patterns(), side).forced_block() {
        let mut moves = BitBoard256::EMPTY;
        // An attacker block must still belong to the forcing vocabulary.
        if side != attacker || attacks(board.patterns(), attacker).test(at) {
            moves.set(at);
        }
        return moves;
    }
    if side == attacker {
        attacks(board.patterns(), attacker)
    } else {
        active.map_or(BitBoard256::EMPTY, |threat| {
            threat.responses(board, attacker)
        })
    }
}

#[cfg(test)]
mod tests;
