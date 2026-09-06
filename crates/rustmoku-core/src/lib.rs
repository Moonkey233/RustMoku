#![forbid(unsafe_code)]

mod domain;
mod game;
mod notation;
mod openings;
mod position;
mod record;
mod symmetry;

pub use domain::{BOARD_SIZE, CELL_COUNT, Move, MoveError, RuleSet, Stone};
pub use game::{Game, GameStatus};
pub use notation::MoveNotationError;
pub use openings::{OPENINGS, Opening};
pub use position::{MoveUndo, Position};
pub use record::RecordError;
pub use symmetry::{CanonicalKeyError, CanonicalPosition, CanonicalPositionKey, Symmetry};
