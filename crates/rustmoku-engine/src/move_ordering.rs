use rustmoku_core::{BOARD_SIZE, CELL_COUNT, Move, Position, Stone};

use crate::move_generation::MoveList;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MovePriority {
    tactical_class: u8,
    is_tt_move: bool,
    local_score: i32,
}

pub(crate) fn order_moves(position: &Position, moves: &mut MoveList, tt_move: Option<Move>) {
    let side = position.side_to_move();
    let mut priorities = [MovePriority::default(); CELL_COUNT];
    for at in moves.iter() {
        priorities[at.index()] = MovePriority {
            tactical_class: if position.would_win(at, side) {
                2
            } else if position.would_win(at, side.opponent()) {
                1
            } else {
                0
            },
            is_tt_move: Some(at) == tt_move,
            local_score: local_score(position, at, side),
        };
    }

    moves.as_mut_slice().sort_unstable_by(|left, right| {
        let left_priority = priorities[left.index()];
        let right_priority = priorities[right.index()];
        right_priority
            .tactical_class
            .cmp(&left_priority.tactical_class)
            .then_with(|| right_priority.is_tt_move.cmp(&left_priority.is_tt_move))
            .then_with(|| right_priority.local_score.cmp(&left_priority.local_score))
            .then_with(|| left.index().cmp(&right.index()))
    });
}

fn local_score(position: &Position, at: Move, side: Stone) -> i32 {
    let mut score = 0;
    for row_delta in -2_isize..=2 {
        for column_delta in -2_isize..=2 {
            if row_delta == 0 && column_delta == 0 {
                continue;
            }
            let Some(row) = at.row().checked_add_signed(row_delta) else {
                continue;
            };
            let Some(column) = at.column().checked_add_signed(column_delta) else {
                continue;
            };
            let Ok(neighbor) = Move::from_row_col(row, column) else {
                continue;
            };
            let distance = row_delta.unsigned_abs().max(column_delta.unsigned_abs());
            score += match (position.cell(neighbor), distance) {
                (Some(stone), 1) if stone == side => 8,
                (Some(_), 1) => 7,
                (Some(stone), 2) if stone == side => 2,
                (Some(_), 2) => 1,
                (None, _) | (Some(_), _) => 0,
            };
        }
    }

    let center = BOARD_SIZE / 2;
    let center_distance = at.row().abs_diff(center) + at.column().abs_diff(center);
    // On a 15 x 15 board, the Manhattan distance from center is at most 14.
    score + (BOARD_SIZE - 1 - center_distance) as i32
}

#[cfg(test)]
mod tests {
    use rustmoku_core::{Move, Position, Stone};

    use super::order_moves;
    use crate::move_generation::generate_candidates;

    fn move_at(row: usize, column: usize) -> Move {
        Move::from_row_col(row, column).expect("test coordinates must be valid")
    }

    fn position_from(moves: &[(usize, usize)]) -> Position {
        let mut position = Position::default();
        for &(row, column) in moves {
            position
                .make_move(move_at(row, column))
                .expect("test sequence must be legal");
        }
        position
    }

    #[test]
    fn tt_move_leads_its_non_tactical_class() {
        let position = position_from(&[(7, 7)]);
        let tt_move = move_at(5, 5);
        let mut moves = generate_candidates(&position);
        order_moves(&position, &mut moves, Some(tt_move));
        assert_eq!(moves.as_slice().first().copied(), Some(tt_move));
    }

    #[test]
    fn immediate_win_outranks_non_tactical_tt_move() {
        let position = position_from(&[
            (7, 3),
            (0, 0),
            (7, 4),
            (0, 2),
            (7, 5),
            (1, 0),
            (7, 6),
            (1, 2),
        ]);
        let mut moves = generate_candidates(&position);
        order_moves(&position, &mut moves, Some(move_at(5, 5)));
        let first = moves.as_slice()[0];
        assert!(position.would_win(first, Stone::Black));
    }

    #[test]
    fn forced_block_outranks_non_tactical_tt_move() {
        let position = position_from(&[
            (7, 2),
            (7, 3),
            (0, 0),
            (7, 4),
            (0, 2),
            (7, 5),
            (1, 0),
            (7, 6),
        ]);
        let mut moves = generate_candidates(&position);
        order_moves(&position, &mut moves, Some(move_at(5, 5)));
        assert_eq!(moves.as_slice().first().copied(), Some(move_at(7, 7)));
    }
}
