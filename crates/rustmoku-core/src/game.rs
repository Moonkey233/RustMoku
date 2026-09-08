use crate::{Move, MoveError, MoveUndo, Position, RuleSet, Stone};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GameStatus {
    #[default]
    Ongoing,
    Won(Stone),
    Draw,
}

#[derive(Debug)]
pub struct Game {
    position: Position,
    status: GameStatus,
    history: Vec<PlayedMove>,
    future: Vec<Move>,
}

#[derive(Debug)]
struct PlayedMove {
    at: Move,
    undo: MoveUndo,
}

impl PartialEq for Game {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position
            && self.status == other.status
            && self.history().eq(other.history())
    }
}
impl Eq for Game {}

impl Game {
    #[must_use]
    pub const fn new(rules: RuleSet) -> Self {
        Self {
            position: Position::new(rules),
            status: GameStatus::Ongoing,
            history: Vec::new(),
            future: Vec::new(),
        }
    }

    #[must_use]
    pub const fn position(&self) -> &Position {
        &self.position
    }

    #[must_use]
    pub const fn status(&self) -> GameStatus {
        self.status
    }

    /// Chronological played moves, including opening/imported moves. Undo tokens
    /// stay private; applications receive only validated move values.
    pub fn history(&self) -> impl ExactSizeIterator<Item = Move> + DoubleEndedIterator + '_ {
        self.history.iter().map(|played| played.at)
    }

    /// Undo one ply, returning its move. Empty games are unchanged.
    pub fn undo(&mut self) -> Option<Move> {
        let played = self.history.pop()?;
        self.position.unmake_move(played.undo);
        self.future.push(played.at);
        // Every recorded move was accepted from an ongoing game.
        self.status = GameStatus::Ongoing;
        Some(played.at)
    }

    /// Undo up to `plies` moves. Returns the actual number removed.
    pub fn undo_plies(&mut self, plies: usize) -> usize {
        let count = plies.min(self.history.len());
        for _ in 0..count {
            self.undo();
        }
        count
    }

    /// Whether at least one move can be replayed from the abandoned future.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Number of moves available for chronological replay.
    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.future.len()
    }

    /// Replay one historical move through the normal legal transition path.
    pub fn redo(&mut self) -> Option<Move> {
        let at = self.future.last().copied()?;
        self.play_move_internal(at, false)
            .expect("private redo timeline must remain legally replayable");
        self.future.pop();
        Some(at)
    }

    /// Redo up to `plies` moves. Returns the actual number replayed.
    pub fn redo_plies(&mut self, plies: usize) -> usize {
        let count = plies.min(self.future.len());
        for _ in 0..count {
            self.redo();
        }
        count
    }

    /// Plays one move and updates the game outcome.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::GameOver`] after a win or draw, or propagates the
    /// position's legality error.
    pub fn play_move(&mut self, at: Move) -> Result<(), MoveError> {
        self.play_move_internal(at, true)
    }

    fn play_move_internal(&mut self, at: Move, clear_future: bool) -> Result<(), MoveError> {
        if self.status != GameStatus::Ongoing {
            return Err(MoveError::GameOver);
        }

        let moved_stone = self.position.side_to_move();
        let undo = self.position.make_move(at)?;
        self.history.push(PlayedMove { at, undo });
        if clear_future {
            self.future.clear();
        }
        self.status = if self.position.winner() == Some(moved_stone) {
            GameStatus::Won(moved_stone)
        } else if self.position.is_full() {
            GameStatus::Draw
        } else {
            GameStatus::Ongoing
        };
        Ok(())
    }
}

impl Default for Game {
    fn default() -> Self {
        Self::new(RuleSet::Freestyle)
    }
}
