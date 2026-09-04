use crate::{Move, MoveError, Position, RuleSet, Stone};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GameStatus {
    #[default]
    Ongoing,
    Won(Stone),
    Draw,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Game {
    position: Position,
    status: GameStatus,
}

impl Game {
    #[must_use]
    pub const fn new(rules: RuleSet) -> Self {
        Self {
            position: Position::new(rules),
            status: GameStatus::Ongoing,
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

    /// Plays one move and updates the game outcome.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::GameOver`] after a win or draw, or propagates the
    /// position's legality error.
    pub fn play_move(&mut self, at: Move) -> Result<(), MoveError> {
        if self.status != GameStatus::Ongoing {
            return Err(MoveError::GameOver);
        }

        let moved_stone = self.position.side_to_move();
        let _undo = self.position.make_move(at)?;
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
