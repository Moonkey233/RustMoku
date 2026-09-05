use rustmoku_core::{CELL_COUNT, Move, Stone};

use crate::{
    PatternState,
    pattern::{ThreatProfile, stone_index},
};

/// Rebuilt for every public search; shared only by its iterations/re-searches.
pub(crate) struct SearchHeuristics {
    history: [[u16; CELL_COUNT]; 2],
    killers: [[Option<Move>; 2]; CELL_COUNT + 1],
}

impl Default for SearchHeuristics {
    fn default() -> Self {
        Self {
            history: [[0; CELL_COUNT]; 2],
            killers: [[None; 2]; CELL_COUNT + 1],
        }
    }
}

impl SearchHeuristics {
    /// A single-ply reduction for the ninth or later genuinely quiet move.
    /// The caller additionally excludes PV windows, hash moves, and forced blocks.
    pub(crate) fn lmr_reduction(
        &self,
        depth: u8,
        index: usize,
        side: Stone,
        at: Move,
        ply: u8,
        patterns: &PatternState,
    ) -> u8 {
        u8::from(
            depth >= 3
                && index >= 8
                && patterns.profile(at, side) == ThreatProfile::Quiet
                && patterns.profile(at, side.opponent()) == ThreatProfile::Quiet
                && self.history(side, at) < 128
                && self.killer_rank(ply, at) == 0,
        )
    }

    pub(crate) fn history(&self, side: Stone, at: Move) -> u16 {
        self.history[stone_index(side)][at.index()]
    }

    pub(crate) fn killer_rank(&self, ply: u8, at: Move) -> u8 {
        match self.killers[usize::from(ply)] {
            [Some(first), _] if first == at => 2,
            [_, Some(second)] if second == at => 1,
            _ => 0,
        }
    }

    pub(crate) fn record_cutoff(
        &mut self,
        side: Stone,
        at: Move,
        depth: u8,
        ply: u8,
        patterns: &PatternState,
    ) {
        if patterns.profile(at, side) >= ThreatProfile::Four
            || patterns.profile(at, side.opponent()) >= ThreatProfile::Four
        {
            return;
        }
        // History gravity bounds values below 2^14 without a board-wide decay.
        let value = &mut self.history[stone_index(side)][at.index()];
        let bonus = u32::from(depth).pow(2).min(1024);
        *value = (u32::from(*value) + bonus - u32::from(*value) * bonus / 16384).min(16383) as u16;
        let killers = &mut self.killers[usize::from(ply)];
        if killers[0] != Some(at) {
            killers[1] = killers[0];
            killers[0] = Some(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustmoku_core::Position;

    #[test]
    fn history_is_bounded_and_killers_are_distinct_and_local() {
        let patterns = PatternState::new(&Position::default());
        let mut heuristics = SearchHeuristics::default();
        let other = Move::from_index(111).unwrap();
        for _ in 0..1000 {
            heuristics.record_cutoff(Stone::Black, Move::CENTER, 255, 3, &patterns);
        }
        assert!(heuristics.history(Stone::Black, Move::CENTER) <= 16383);
        heuristics.record_cutoff(Stone::Black, other, 4, 3, &patterns);
        heuristics.record_cutoff(Stone::Black, other, 4, 3, &patterns);
        assert_eq!(heuristics.killer_rank(3, other), 2);
        assert_eq!(heuristics.killer_rank(3, Move::CENTER), 1);
        assert_eq!(heuristics.killer_rank(2, other), 0);
        assert_eq!(heuristics.history(Stone::White, other), 0);
        assert_eq!(SearchHeuristics::default().killer_rank(3, other), 0);
    }
}
