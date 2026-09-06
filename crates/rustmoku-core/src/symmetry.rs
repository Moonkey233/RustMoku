use std::fmt;

use crate::{BOARD_SIZE, CELL_COUNT, Move, Position, Stone};

/// The eight symmetries of the square board, in stable canonical order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Symmetry {
    #[default]
    Identity,
    Rotate90,
    Rotate180,
    Rotate270,
    MirrorVertical,
    MirrorHorizontal,
    MirrorMainDiagonal,
    MirrorAntiDiagonal,
}

impl Symmetry {
    pub const ALL: [Self; 8] = [
        Self::Identity,
        Self::Rotate90,
        Self::Rotate180,
        Self::Rotate270,
        Self::MirrorVertical,
        Self::MirrorHorizontal,
        Self::MirrorMainDiagonal,
        Self::MirrorAntiDiagonal,
    ];

    /// Transforms a validated board location without changing its validity.
    #[must_use]
    pub const fn transform(self, at: Move) -> Move {
        let last = BOARD_SIZE - 1;
        let row = at.row();
        let column = at.column();
        let (row, column) = match self {
            Self::Identity => (row, column),
            Self::Rotate90 => (column, last - row),
            Self::Rotate180 => (last - row, last - column),
            Self::Rotate270 => (last - column, row),
            Self::MirrorVertical => (row, last - column),
            Self::MirrorHorizontal => (last - row, column),
            Self::MirrorMainDiagonal => (column, row),
            Self::MirrorAntiDiagonal => (last - column, last - row),
        };
        match Move::from_row_col(row, column) {
            Ok(at) => at,
            Err(_) => unreachable!(),
        }
    }

    #[must_use]
    pub const fn inverse(self) -> Self {
        match self {
            Self::Rotate90 => Self::Rotate270,
            Self::Rotate270 => Self::Rotate90,
            other => other,
        }
    }
}

/// Collision-free packed board state used for persistence and canonical identity.
///
/// Each cell occupies two bits (empty, Black, White); the final byte records the
/// side to move. RuleSet remains an explicit part of the surrounding context.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalPositionKey([u8; Self::BYTE_LEN]);

impl CanonicalPositionKey {
    pub const BYTE_LEN: usize = CELL_COUNT.div_ceil(4) + 1;

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::BYTE_LEN] {
        &self.0
    }

    /// Rebuilds a key while rejecting encodings that cannot represent a board.
    pub fn from_bytes(bytes: [u8; Self::BYTE_LEN]) -> Result<Self, CanonicalKeyError> {
        let packed_cells = Self::BYTE_LEN - 1;
        for index in 0..CELL_COUNT {
            let shift = (3 - index % 4) * 2;
            if ((bytes[index / 4] >> shift) & 0b11) == 0b11 {
                return Err(CanonicalKeyError::InvalidCell);
            }
        }
        let used_cells = CELL_COUNT % 4;
        let padding_bits = (4 - used_cells) * 2;
        if used_cells != 0 && bytes[packed_cells - 1] & ((1_u8 << padding_bits) - 1) != 0 {
            return Err(CanonicalKeyError::NonZeroPadding);
        }
        if bytes[packed_cells] > 1 {
            return Err(CanonicalKeyError::InvalidSide);
        }
        Ok(Self(bytes))
    }

    fn from_position_with(position: &Position, symmetry: Symmetry) -> Self {
        let mut bytes = [0_u8; Self::BYTE_LEN];
        for at in Move::all() {
            let code = match position.cell(at) {
                None => 0,
                Some(Stone::Black) => 1,
                Some(Stone::White) => 2,
            };
            let transformed = symmetry.transform(at).index();
            bytes[transformed / 4] |= code << ((3 - transformed % 4) * 2);
        }
        bytes[Self::BYTE_LEN - 1] = match position.side_to_move() {
            Stone::Black => 0,
            Stone::White => 1,
        };
        Self(bytes)
    }
}

impl fmt::Debug for CanonicalPositionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CanonicalPositionKey")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalKeyError {
    InvalidCell,
    NonZeroPadding,
    InvalidSide,
}

impl fmt::Display for CanonicalKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidCell => "packed position contains the reserved cell value",
            Self::NonZeroPadding => "packed position contains non-zero padding",
            Self::InvalidSide => "packed position has an invalid side-to-move tag",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for CanonicalKeyError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanonicalPosition {
    key: CanonicalPositionKey,
    original_to_canonical: Symmetry,
}

impl CanonicalPosition {
    #[must_use]
    pub fn new(position: &Position) -> Self {
        let mut best_symmetry = Symmetry::Identity;
        let mut best_key = CanonicalPositionKey::from_position_with(position, best_symmetry);
        for symmetry in Symmetry::ALL.into_iter().skip(1) {
            let key = CanonicalPositionKey::from_position_with(position, symmetry);
            if key < best_key {
                best_key = key;
                best_symmetry = symmetry;
            }
        }
        Self {
            key: best_key,
            original_to_canonical: best_symmetry,
        }
    }

    #[must_use]
    pub const fn key(self) -> CanonicalPositionKey {
        self.key
    }

    #[must_use]
    pub const fn original_to_canonical(self) -> Symmetry {
        self.original_to_canonical
    }

    #[must_use]
    pub fn move_to_canonical(self, at: Move) -> Move {
        self.original_to_canonical.transform(at)
    }

    #[must_use]
    pub fn move_to_original(self, at: Move) -> Move {
        self.original_to_canonical.inverse().transform(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Game, RuleSet};

    #[test]
    fn transforms_round_trip_centers_and_corners() {
        let corner = Move::from_index(0).unwrap();
        let expected = [0, 14, 224, 210, 14, 210, 0, 224];
        for (symmetry, expected) in Symmetry::ALL.into_iter().zip(expected) {
            assert_eq!(symmetry.transform(corner).index(), expected);
            assert_eq!(symmetry.transform(Move::CENTER), Move::CENTER);
        }
        for symmetry in Symmetry::ALL {
            for index in [0, 14, 112, 210, 224] {
                let at = Move::from_index(index).unwrap();
                assert_eq!(symmetry.inverse().transform(symmetry.transform(at)), at);
            }
        }
    }

    #[test]
    fn every_orientation_has_one_canonical_key() {
        let moves = [112, 97, 128, 113, 142, 83, 126];
        let mut reference = None;
        for symmetry in Symmetry::ALL {
            let mut game = Game::new(RuleSet::Freestyle);
            for index in moves {
                game.play_move(symmetry.transform(Move::from_index(index).unwrap()))
                    .unwrap();
            }
            let key = CanonicalPosition::new(game.position()).key();
            assert!(reference.is_none_or(|expected| expected == key));
            reference = Some(key);
        }
    }
}
