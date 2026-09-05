use rustmoku_core::{Move, MoveError, MoveUndo, Position, Stone};

use crate::{
    PatternState, bitboard::BitBoard256, candidate_frontier::CandidateFrontier,
    move_generation::MoveList, pattern_state::PatternUndo, zobrist::PositionKey,
};

/// Reversible board sidecars shared by classical search and tactical proofs.
/// Deliberately has no evaluator or evaluator accumulator.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BoardState {
    position: Position,
    key: PositionKey,
    frontier: CandidateFrontier,
    patterns: PatternState,
}

pub(crate) struct BoardUndo {
    position: MoveUndo,
    pattern: PatternUndo,
    at: Move,
    stone: Stone,
}

impl BoardState {
    pub(crate) fn new(position: &Position) -> Self {
        Self {
            // One intentional working copy at the search root; never per node.
            position: position.clone(),
            key: PositionKey::from_position(position),
            frontier: CandidateFrontier::new(position),
            patterns: PatternState::new(position),
        }
    }

    pub(crate) const fn position(&self) -> &Position {
        &self.position
    }
    pub(crate) const fn key(&self) -> PositionKey {
        self.key
    }
    pub(crate) fn patterns(&self) -> &PatternState {
        &self.patterns
    }

    pub(crate) fn candidate_bits(&self) -> BitBoard256 {
        if self.position.winner().is_some() || self.position.is_full() {
            BitBoard256::EMPTY
        } else {
            self.frontier.candidate_bits()
        }
    }

    pub(crate) fn candidates(&self) -> MoveList {
        if self.position.winner().is_some() || self.position.is_full() {
            MoveList::new()
        } else {
            self.frontier.candidates()
        }
    }

    pub(crate) fn make_move(&mut self, at: Move) -> Result<BoardUndo, MoveError> {
        let stone = self.position.side_to_move();
        let position = self.position.make_move(at)?;
        self.frontier.make_move(at);
        let pattern = self.patterns.make_move(at, stone);
        self.key = self.key.toggle_move(at, stone);
        debug_assert_eq!(self.key, PositionKey::from_position(&self.position));
        Ok(BoardUndo {
            position,
            pattern,
            at,
            stone,
        })
    }

    pub(crate) fn unmake_move(&mut self, undo: BoardUndo) {
        self.patterns.unmake_move(undo.pattern);
        self.position.unmake_move(undo.position);
        self.frontier.unmake_move(undo.at);
        self.key = self.key.toggle_move(undo.at, undo.stone);
        debug_assert_eq!(self.key, PositionKey::from_position(&self.position));
    }

    #[cfg(test)]
    pub(crate) fn assert_consistent(&self) {
        assert_eq!(self, &Self::new(&self.position));
        assert_eq!(self.patterns(), &PatternState::reference(&self.position));
        assert_eq!(
            self.candidates(),
            crate::move_generation::generate_candidates(&self.position)
        );
    }
}
