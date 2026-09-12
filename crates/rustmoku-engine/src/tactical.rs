use rustmoku_core::{Move, Stone};
mod threat;
pub(crate) use threat::{ThreatDescriptor, attacks};

use crate::{
    PatternState, bitboard::BitBoard256, pattern::ThreatProfile, principal_variation::PvTable,
    score::MATE_SCORE,
};

/// Potential forcing placements; proof search must recheck the resulting board.
pub(crate) fn forcing_moves(patterns: &PatternState, side: Stone) -> BitBoard256 {
    ThreatResolver::new(patterns, side).forcing()
}

/// Separates exact immediate obligations from structural candidate hints.
/// Profiles include double Four, FourThree and double OpenThree; none of these
/// potential placements is promoted to a proof without checking its continuations.
pub(crate) struct ThreatResolver<'a> {
    patterns: &'a PatternState,
    side: Stone,
}

impl<'a> ThreatResolver<'a> {
    pub(crate) fn new(patterns: &'a PatternState, side: Stone) -> Self {
        Self { patterns, side }
    }

    pub(crate) fn forcing(&self) -> BitBoard256 {
        self.patterns.moves_at_least(self.side, ThreatProfile::Four)
    }

    /// Both attacks and counter-threat/defense candidates, not an exact defense set.
    pub(crate) fn hints(&self) -> BitBoard256 {
        self.patterns
            .moves_at_least(self.side, ThreatProfile::OpenThree)
            .union(
                self.patterns
                    .moves_at_least(self.side.opponent(), ThreatProfile::OpenThree),
            )
    }

    pub(crate) fn immediate(&self) -> ImmediateTactic {
        let patterns = self.patterns;
        let side = self.side;
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
        ImmediateTactic::Loss {
            at: first,
            reply: second,
        }
    }
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
    // Exact loss-in-two: resist at a real winning point before applying the
    // canonical index tie-break. Another immediate point remains terminal.
    ThreatResolver::new(patterns, side).immediate()
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
