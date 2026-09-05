//! Hand-authored Freestyle test starts, not an empirically balanced opening book.
use crate::{Game, Move, MoveError, RuleSet};

pub struct Opening {
    pub id: &'static str,
    pub name: &'static str,
    pub rules: RuleSet,
    pub moves: &'static [Move],
}

impl Opening {
    /// Instantiate only through normal legal Game transitions.
    pub fn game(&self) -> Result<Game, MoveError> {
        let mut game = Game::new(self.rules);
        for &at in self.moves {
            game.play_move(at)?;
        }
        Ok(game)
    }
}

// Compile-time validated internal indices; presentation always uses Move's codec.
const fn at(index: usize) -> Move {
    match Move::from_index(index) {
        Ok(at) => at,
        Err(_) => panic!("invalid built-in opening coordinate"),
    }
}

/// Stable order supports deterministic cycling and identical Arena paired legs.
pub const OPENINGS: &[Opening] = &[
    Opening {
        id: "diagonal",
        name: "Central diagonal",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(97), at(128), at(113)],
    },
    Opening {
        id: "cross",
        name: "Central cross",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(113), at(127), at(98)],
    },
    Opening {
        id: "corners",
        name: "Nearby corners",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(96), at(98), at(126)],
    },
    Opening {
        id: "lanes",
        name: "Separate lanes",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(97), at(111), at(128), at(142), at(82)],
    },
    Opening {
        id: "contact",
        name: "Close contact",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(113), at(97)],
    },
    Opening {
        id: "gap",
        name: "One-point gap",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(114), at(110), at(99)],
    },
    Opening {
        id: "stair",
        name: "Rising stair",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(98), at(128), at(114), at(144)],
    },
    Opening {
        id: "wide",
        name: "Wide center",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(80), at(144), at(84), at(140), at(82)],
    },
    Opening {
        id: "north",
        name: "Northern approach",
        rules: RuleSet::Freestyle,
        moves: &[at(52), at(67), at(53), at(68)],
    },
    Opening {
        id: "edge",
        name: "Left-edge study",
        rules: RuleSet::Freestyle,
        moves: &[at(105), at(106), at(120), at(121)],
    },
    Opening {
        id: "south",
        name: "Southern diagonal",
        rules: RuleSet::Freestyle,
        moves: &[at(172), at(157), at(188), at(173), at(156)],
    },
    Opening {
        id: "broken",
        name: "Broken lines",
        rules: RuleSet::Freestyle,
        moves: &[at(112), at(97), at(114), at(99), at(127), at(83)],
    },
];
