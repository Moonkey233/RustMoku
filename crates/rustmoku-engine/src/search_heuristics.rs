use rustmoku_core::{CELL_COUNT, Move, Stone};

use crate::{
    PatternState,
    pattern::{ThreatProfile, stone_index},
    search_params,
};

const CONTINUATION_SIZE: usize = 2 * CELL_COUNT * CELL_COUNT;
const STACK_SIZE: usize = CELL_COUNT + 2;

#[derive(Clone, Copy, Default)]
struct StackEntry {
    current_move: Option<Move>,
    static_eval: Option<i32>,
    cut_node: bool,
    extensions: u8,
}

/// Rebuilt for every public search and fully private to one Lazy-SMP worker.
/// Large continuation tables are allocated once here, never in recursion.
pub(crate) struct SearchHeuristics {
    history: [[i16; CELL_COUNT]; 2],
    killers: [[Option<Move>; 2]; STACK_SIZE],
    countermoves: [[Option<Move>; CELL_COUNT]; 2],
    continuation_1: Box<[i16]>,
    continuation_2: Box<[i16]>,
    stack: [StackEntry; STACK_SIZE],
}

impl Default for SearchHeuristics {
    fn default() -> Self {
        Self {
            history: [[0; CELL_COUNT]; 2],
            killers: [[None; 2]; STACK_SIZE],
            countermoves: [[None; CELL_COUNT]; 2],
            continuation_1: vec![0; CONTINUATION_SIZE].into_boxed_slice(),
            continuation_2: vec![0; CONTINUATION_SIZE].into_boxed_slice(),
            stack: [StackEntry::default(); STACK_SIZE],
        }
    }
}

impl SearchHeuristics {
    pub(crate) fn begin_root(&mut self) {
        self.stack[0] = StackEntry::default();
    }

    pub(crate) fn begin_node(&mut self, ply: u8) {
        self.stack[usize::from(ply)].static_eval = None;
    }

    pub(crate) fn set_child(&mut self, ply: u8, at: Move, cut_node: bool, extensions: u8) {
        self.stack[usize::from(ply)] = StackEntry {
            current_move: Some(at),
            static_eval: None,
            cut_node,
            extensions,
        };
    }

    pub(crate) fn previous_moves(&self, ply: u8) -> (Option<Move>, Option<Move>) {
        let ply = usize::from(ply);
        (
            self.stack[ply].current_move,
            ply.checked_sub(1)
                .and_then(|previous| self.stack[previous].current_move),
        )
    }

    pub(crate) fn cut_node(&self, ply: u8) -> bool {
        self.stack[usize::from(ply)].cut_node
    }

    pub(crate) fn extensions(&self, ply: u8) -> u8 {
        self.stack[usize::from(ply)].extensions
    }

    pub(crate) fn static_eval(&self, ply: u8) -> Option<i32> {
        self.stack[usize::from(ply)].static_eval
    }

    pub(crate) fn set_static_eval(&mut self, ply: u8, score: i32) {
        self.stack[usize::from(ply)].static_eval = Some(score);
    }

    pub(crate) fn history(&self, side: Stone, at: Move) -> i16 {
        self.history[stone_index(side)][at.index()]
    }

    pub(crate) fn killer_rank(&self, ply: u8, at: Move) -> u8 {
        match self.killers[usize::from(ply)] {
            [Some(first), _] if first == at => 2,
            [_, Some(second)] if second == at => 1,
            _ => 0,
        }
    }

    pub(crate) fn is_countermove(&self, side: Stone, at: Move, previous: Option<Move>) -> bool {
        previous
            .is_some_and(|prior| self.countermoves[stone_index(side)][prior.index()] == Some(at))
    }

    pub(crate) fn contextual_score(
        &self,
        side: Stone,
        at: Move,
        previous: Option<Move>,
        two_back: Option<Move>,
    ) -> i16 {
        let mut score = i32::from(self.history(side, at));
        if let Some(prior) = previous {
            score += i32::from(self.continuation_1[continuation_index(side, prior, at)]);
        }
        if let Some(prior) = two_back {
            score += i32::from(self.continuation_2[continuation_index(side, prior, at)]);
        }
        score.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
    }

    pub(crate) fn is_quiet(patterns: &PatternState, side: Stone, at: Move) -> bool {
        patterns.profile(at, side) == ThreatProfile::Quiet
            && patterns.profile(at, side.opponent()) == ThreatProfile::Quiet
    }

    pub(crate) fn is_strong_context(
        &self,
        side: Stone,
        at: Move,
        ply: u8,
        previous: Option<Move>,
        two_back: Option<Move>,
    ) -> bool {
        self.killer_rank(ply, at) != 0
            || self.is_countermove(side, at, previous)
            || self.contextual_score(side, at, previous, two_back) >= search_params::STRONG_HISTORY
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn adaptive_lmr_reduction(
        &self,
        depth: u8,
        index: usize,
        side: Stone,
        at: Move,
        ply: u8,
        previous: Option<Move>,
        two_back: Option<Move>,
        patterns: &PatternState,
    ) -> u8 {
        let own = patterns.profile(at, side);
        let opponent = patterns.profile(at, side.opponent());
        if depth < search_params::LMR_MIN_DEPTH
            || index < search_params::LMR_MIN_INDEX
            || own != ThreatProfile::Quiet
            || opponent != ThreatProfile::Quiet
            || self.is_strong_context(side, at, ply, previous, two_back)
        {
            return 0;
        }
        let mut reduction = search_params::lmr_base(depth, index);
        if self.cut_node(ply) && reduction < depth - 1 {
            reduction += 1;
        }
        reduction.min(depth - 1)
    }

    #[cfg(test)]
    pub(crate) fn lmr_reduction(
        &self,
        depth: u8,
        index: usize,
        side: Stone,
        at: Move,
        ply: u8,
        patterns: &PatternState,
    ) -> u8 {
        self.adaptive_lmr_reduction(depth, index, side, at, ply, None, None, patterns)
    }

    #[cfg(test)]
    pub(crate) fn record_cutoff(
        &mut self,
        side: Stone,
        at: Move,
        depth: u8,
        ply: u8,
        patterns: &PatternState,
    ) {
        self.record_cutoff_with_context(true, side, at, depth, ply, None, None, &[], patterns);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_cutoff_with_context(
        &mut self,
        lower_verified: bool,
        side: Stone,
        at: Move,
        depth: u8,
        ply: u8,
        previous: Option<Move>,
        two_back: Option<Move>,
        searched_quiets: &[Move],
        patterns: &PatternState,
    ) {
        if !lower_verified || !Self::is_quiet(patterns, side, at) {
            return;
        }
        let bonus = search_params::history_bonus(depth);
        self.update_histories(side, at, previous, two_back, bonus);
        let malus = -search_params::history_malus(depth);
        for &quiet in searched_quiets {
            if quiet != at {
                self.update_histories(side, quiet, previous, two_back, malus);
            }
        }
        if let Some(prior) = previous {
            self.countermoves[stone_index(side)][prior.index()] = Some(at);
        }
        let killers = &mut self.killers[usize::from(ply)];
        if killers[0] != Some(at) {
            killers[1] = killers[0];
            killers[0] = Some(at);
        }
    }

    fn update_histories(
        &mut self,
        side: Stone,
        at: Move,
        previous: Option<Move>,
        two_back: Option<Move>,
        bonus: i32,
    ) {
        gravity_update(&mut self.history[stone_index(side)][at.index()], bonus);
        if let Some(prior) = previous {
            gravity_update(
                &mut self.continuation_1[continuation_index(side, prior, at)],
                bonus,
            );
        }
        if let Some(prior) = two_back {
            gravity_update(
                &mut self.continuation_2[continuation_index(side, prior, at)],
                bonus,
            );
        }
    }
}

fn continuation_index(side: Stone, prior: Move, at: Move) -> usize {
    (stone_index(side) * CELL_COUNT + prior.index()) * CELL_COUNT + at.index()
}

fn gravity_update(value: &mut i16, bonus: i32) {
    let bounded = bonus.clamp(-search_params::HISTORY_MAX, search_params::HISTORY_MAX);
    let current = i32::from(*value);
    let updated = current + bounded - current * bounded.abs() / search_params::HISTORY_MAX;
    *value = updated.clamp(-search_params::HISTORY_MAX, search_params::HISTORY_MAX) as i16;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustmoku_core::Position;

    #[test]
    fn signed_history_gravity_malus_and_context_are_bounded() {
        let patterns = PatternState::new(&Position::default());
        let mut heuristics = SearchHeuristics::default();
        let reply = Move::CENTER;
        let prior = Move::from_index(111).unwrap();
        let older = Move::from_index(110).unwrap();
        let failed = Move::from_index(109).unwrap();
        for _ in 0..1000 {
            heuristics.record_cutoff_with_context(
                true,
                Stone::Black,
                reply,
                12,
                3,
                Some(prior),
                Some(older),
                &[failed],
                &patterns,
            );
        }
        assert!(heuristics.history(Stone::Black, reply) <= search_params::HISTORY_MAX as i16);
        assert!(heuristics.history(Stone::Black, failed) < 0);
        assert!(heuristics.is_countermove(Stone::Black, reply, Some(prior)));
        assert!(heuristics.contextual_score(Stone::Black, reply, Some(prior), Some(older)) > 0);
        assert!(heuristics.contextual_score(Stone::Black, failed, Some(prior), Some(older)) < 0);
    }

    #[test]
    fn killers_are_distinct_and_local() {
        let patterns = PatternState::new(&Position::default());
        let mut heuristics = SearchHeuristics::default();
        let other = Move::from_index(111).unwrap();
        heuristics.record_cutoff(Stone::Black, Move::CENTER, 4, 3, &patterns);
        heuristics.record_cutoff(Stone::Black, other, 4, 3, &patterns);
        assert_eq!(heuristics.killer_rank(3, other), 2);
        assert_eq!(heuristics.killer_rank(3, Move::CENTER), 1);
        assert_eq!(heuristics.killer_rank(2, other), 0);
        assert_eq!(heuristics.history(Stone::White, other), 0);
    }

    #[test]
    fn unverified_or_non_quiet_cutoff_does_not_train_ordinary_heuristics() {
        let patterns = PatternState::new(&Position::default());
        let mut heuristics = SearchHeuristics::default();
        let reply = Move::CENTER;
        let prior = Move::from_index(111).unwrap();
        let older = Move::from_index(110).unwrap();
        let failed = Move::from_index(109).unwrap();
        heuristics.record_cutoff_with_context(
            false,
            Stone::Black,
            reply,
            8,
            3,
            Some(prior),
            Some(older),
            &[failed],
            &patterns,
        );
        assert_eq!(heuristics.history(Stone::Black, reply), 0);
        assert_eq!(heuristics.history(Stone::Black, failed), 0);
        assert!(!heuristics.is_countermove(Stone::Black, reply, Some(prior)));
        assert_eq!(heuristics.killer_rank(3, reply), 0);

        let mut position = Position::default();
        for index in [110, 0, 111, 2, 112, 15] {
            position
                .make_move(Move::from_index(index).unwrap())
                .unwrap();
        }
        let patterns = PatternState::new(&position);
        let side = position.side_to_move();
        let tactical = patterns
            .empty_cells()
            .iter()
            .find(|&at| !SearchHeuristics::is_quiet(&patterns, side, at))
            .expect("fixture must contain a tactical candidate");
        heuristics.record_cutoff_with_context(
            true,
            side,
            tactical,
            8,
            3,
            Some(prior),
            Some(older),
            &[],
            &patterns,
        );
        assert_eq!(heuristics.history(side, tactical), 0);
        assert!(!heuristics.is_countermove(side, tactical, Some(prior)));
        assert_eq!(heuristics.killer_rank(3, tactical), 0);
    }
}
