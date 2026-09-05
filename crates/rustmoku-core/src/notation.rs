//! Authoritative human coordinates. Internal row/index geometry is unchanged.
use crate::{BOARD_SIZE, Move};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveNotationError;

impl fmt::Display for MoveNotationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected A1..O15 (including I; A1 is bottom-left)")
    }
}
impl std::error::Error for MoveNotationError {}

impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let column = char::from(b'A' + self.column() as u8);
        write!(f, "{column}{}", BOARD_SIZE - self.row())
    }
}

impl FromStr for Move {
    type Err = MoveNotationError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bytes = text.as_bytes();
        if !(2..=3).contains(&bytes.len()) {
            return Err(MoveNotationError);
        }
        let column = bytes[0].to_ascii_uppercase();
        if !(b'A'..=b'O').contains(&column)
            || bytes[1] == b'0'
            || !bytes[1..].iter().all(u8::is_ascii_digit)
        {
            return Err(MoveNotationError);
        }
        let row = text[1..].parse::<usize>().map_err(|_| MoveNotationError)?;
        if !(1..=BOARD_SIZE).contains(&row) {
            return Err(MoveNotationError);
        }
        Self::from_row_col(BOARD_SIZE - row, usize::from(column - b'A'))
            .map_err(|_| MoveNotationError)
    }
}
