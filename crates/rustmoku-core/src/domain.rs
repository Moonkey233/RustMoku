use std::fmt;

pub const BOARD_SIZE: usize = 15;
pub const CELL_COUNT: usize = BOARD_SIZE * BOARD_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stone {
    Black,
    White,
}

impl Stone {
    #[must_use]
    pub const fn opponent(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuleSet {
    #[default]
    Freestyle,
}

/// A validated location on the 15 x 15 board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Move(u8);

impl Move {
    pub const CENTER: Self = Self(112);

    /// Creates a move from zero-based row and column coordinates.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::OutOfBounds`] when either coordinate is outside
    /// the board.
    pub fn from_row_col(row: usize, column: usize) -> Result<Self, MoveError> {
        if row >= BOARD_SIZE || column >= BOARD_SIZE {
            return Err(MoveError::OutOfBounds { row, column });
        }

        // A valid board index is at most 224 and therefore always fits in u8.
        Ok(Self((row * BOARD_SIZE + column) as u8))
    }

    /// Creates a move from a zero-based board index.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::IndexOutOfBounds`] for indices outside 0..225.
    pub fn from_index(index: usize) -> Result<Self, MoveError> {
        if index >= CELL_COUNT {
            return Err(MoveError::IndexOutOfBounds { index });
        }

        // The range check above proves the conversion is lossless.
        Ok(Self(index as u8))
    }

    #[must_use]
    pub const fn row(self) -> usize {
        self.index() / BOARD_SIZE
    }

    #[must_use]
    pub const fn column(self) -> usize {
        self.index() % BOARD_SIZE
    }

    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    pub fn all() -> impl ExactSizeIterator<Item = Self> + DoubleEndedIterator {
        (0_u8..225).map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveError {
    OutOfBounds { row: usize, column: usize },
    IndexOutOfBounds { index: usize },
    Occupied { at: Move },
    GameOver,
}

impl fmt::Display for MoveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { row, column } => {
                write!(
                    formatter,
                    "board coordinates ({row}, {column}) are out of bounds"
                )
            }
            Self::IndexOutOfBounds { index } => {
                write!(formatter, "board index {index} is out of bounds")
            }
            Self::Occupied { at } => write!(
                formatter,
                "board location ({}, {}) is occupied",
                at.row(),
                at.column()
            ),
            Self::GameOver => formatter.write_str("the game is already over"),
        }
    }
}

impl std::error::Error for MoveError {}
