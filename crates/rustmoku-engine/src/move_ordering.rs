use rustmoku_core::{BOARD_SIZE, CELL_COUNT, Move, Position, Stone};

use crate::MoveList;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MovePriority {
    tactical_class: u8,
    local_score: i32,
}

pub(crate) fn order_moves(position: &Position, moves: &mut MoveList) {
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
            local_score: local_score(position, at, side),
        };
    }

    // Selection sort is allocation-free and bounded by the 225-cell board.
    for destination in 0..moves.len() {
        let mut best = destination;
        for candidate in (destination + 1)..moves.len() {
            let Some(candidate_move) = moves.get(candidate) else {
                continue;
            };
            let Some(best_move) = moves.get(best) else {
                continue;
            };
            if comes_before(
                candidate_move,
                priorities[candidate_move.index()],
                best_move,
                priorities[best_move.index()],
            ) {
                best = candidate;
            }
        }
        moves.swap(destination, best);
    }
}

fn comes_before(
    candidate: Move,
    candidate_priority: MovePriority,
    current: Move,
    current_priority: MovePriority,
) -> bool {
    candidate_priority.tactical_class > current_priority.tactical_class
        || (candidate_priority.tactical_class == current_priority.tactical_class
            && (candidate_priority.local_score > current_priority.local_score
                || (candidate_priority.local_score == current_priority.local_score
                    && candidate.index() < current.index())))
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
    score + i32::try_from(BOARD_SIZE - 1 - center_distance).unwrap_or_default()
}
