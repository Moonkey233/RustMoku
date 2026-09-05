use crate::bitboard::MOVES;
use rustmoku_core::{CELL_COUNT, Move};

pub(crate) const DIRECTIONS: [(isize, isize); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];
pub(crate) const OFFSETS: [isize; 8] = [-4, -3, -2, -1, 1, 2, 3, 4];

pub(crate) static LINE_CELLS: [[[Option<Move>; 8]; 4]; CELL_COUNT] = line_cells();
pub(crate) static LINE_INFLUENCES: [Influences; CELL_COUNT] = line_influences();
pub(crate) static CENTER_BIAS: [u8; CELL_COUNT] = center_bias();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Influence {
    pub(crate) center: Move,
    pub(crate) direction: u8,
    pub(crate) shift: u8,
}

pub(crate) struct Influences {
    entries: [Influence; 32],
    len: u8,
}

impl Influences {
    pub(crate) fn iter(&self) -> impl Iterator<Item = Influence> + '_ {
        self.entries[..usize::from(self.len)].iter().copied()
    }
}

const fn line_cells() -> [[[Option<Move>; 8]; 4]; CELL_COUNT] {
    let mut cells = [[[None; 8]; 4]; CELL_COUNT];
    let mut index = 0;
    while index < CELL_COUNT {
        let center = MOVES[index];
        let mut direction = 0;
        while direction < 4 {
            let (dr, dc) = DIRECTIONS[direction];
            let mut field = 0;
            while field < 8 {
                let row = center.row() as isize + dr * OFFSETS[field];
                let column = center.column() as isize + dc * OFFSETS[field];
                if row >= 0 && column >= 0 {
                    cells[index][direction][field] =
                        match Move::from_row_col(row as usize, column as usize) {
                            Ok(at) => Some(at),
                            Err(_) => None,
                        };
                }
                field += 1;
            }
            direction += 1;
        }
        index += 1;
    }
    cells
}

const fn line_influences() -> [Influences; CELL_COUNT] {
    let mut influences = [const {
        Influences {
            entries: [Influence {
                center: Move::CENTER,
                direction: 0,
                shift: 0,
            }; 32],
            len: 0,
        }
    }; CELL_COUNT];
    let mut center = 0;
    while center < CELL_COUNT {
        let mut direction = 0;
        while direction < 4 {
            let mut field = 0;
            while field < 8 {
                if let Some(at) = LINE_CELLS[center][direction][field] {
                    let list = &mut influences[at.index()];
                    list.entries[list.len as usize] = Influence {
                        center: MOVES[center],
                        direction: direction as u8,
                        shift: (field * 2) as u8,
                    };
                    list.len += 1;
                }
                field += 1;
            }
            direction += 1;
        }
        center += 1;
    }
    influences
}

const fn center_bias() -> [u8; CELL_COUNT] {
    let mut biases = [0; CELL_COUNT];
    let mut index = 0;
    while index < CELL_COUNT {
        let at = MOVES[index];
        biases[index] = (14 - at.row().abs_diff(7) - at.column().abs_diff(7)) as u8;
        index += 1;
    }
    biases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_and_influences_are_exact_inverses_with_unique_centers() {
        for at in Move::all() {
            let mut seen = [false; CELL_COUNT];
            for influence in LINE_INFLUENCES[at.index()].iter() {
                assert!(!seen[influence.center.index()]);
                seen[influence.center.index()] = true;
                assert_eq!(
                    LINE_CELLS[influence.center.index()][usize::from(influence.direction)]
                        [usize::from(influence.shift / 2)],
                    Some(at)
                );
            }
            let expected = Move::all()
                .filter(|center| {
                    *center != at
                        && DIRECTIONS.iter().any(|&(dr, dc)| {
                            OFFSETS.iter().any(|&distance| {
                                center.row() as isize + dr * distance == at.row() as isize
                                    && center.column() as isize + dc * distance
                                        == at.column() as isize
                            })
                        })
                })
                .count();
            assert_eq!(LINE_INFLUENCES[at.index()].iter().count(), expected);
            assert_eq!(
                CENTER_BIAS[at.index()] as usize,
                14 - at.row().abs_diff(7) - at.column().abs_diff(7)
            );
        }
        assert_eq!(LINE_INFLUENCES[112].iter().count(), 32);
    }
}
