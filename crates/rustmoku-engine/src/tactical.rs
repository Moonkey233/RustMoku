use rustmoku_core::{Move, Stone};

use crate::{
    PatternState, bitboard::BitBoard256, pattern::ThreatProfile, principal_variation::PvTable,
    score::MATE_SCORE,
};

/// Potential forcing placements; proof search must recheck the resulting board.
pub(crate) fn forcing_moves(patterns: &PatternState, side: Stone) -> BitBoard256 {
    patterns.moves_at_least(side, ThreatProfile::Four)
}

/// Exact Freestyle facts, independent of evaluation and nominal search depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImmediateTactic {
    None,
    Win(Move),
    ForcedBlock(Move),
    Loss { at: Move, reply: Move },
}

pub(crate) fn immediate_tactic(patterns: &PatternState, side: Stone) -> ImmediateTactic {
    if let Some(at) = patterns.winning_moves(side).iter().next() {
        return ImmediateTactic::Win(at);
    }
    let mut threats = patterns.winning_moves(side.opponent()).iter();
    let Some(first) = threats.next() else {
        return ImmediateTactic::None;
    };
    let Some(second) = threats.next() else {
        return ImmediateTactic::ForcedBlock(first);
    };
    // Exact loss-in-two: resist at a real winning point before applying the
    // canonical index tie-break. Another immediate point remains terminal.
    ImmediateTactic::Loss {
        at: first,
        reply: second,
    }
}

impl ImmediateTactic {
    pub(crate) fn forced_block(self) -> Option<Move> {
        if let Self::ForcedBlock(at) = self {
            Some(at)
        } else {
            None
        }
    }

    /// Emits a legal proof prefix without making a copy or visiting child nodes.
    /// Callers must check terminal positions before requesting a tactical fact.
    pub(crate) fn resolve(
        self,
        ply: u8,
        pv: &mut PvTable,
        seldepth: &mut u8,
    ) -> Option<(Move, i32)> {
        match self {
            Self::Win(at) => {
                pv.clear(ply + 1);
                pv.update(ply, at);
                *seldepth = (*seldepth).max(ply + 1);
                Some((at, MATE_SCORE - i32::from(ply) - 1))
            }
            Self::Loss { at, reply } => {
                pv.clear(ply + 2);
                pv.update(ply + 1, reply);
                pv.update(ply, at);
                *seldepth = (*seldepth).max(ply + 2);
                Some((at, -MATE_SCORE + i32::from(ply) + 2))
            }
            Self::None | Self::ForcedBlock(_) => None,
        }
    }
}
