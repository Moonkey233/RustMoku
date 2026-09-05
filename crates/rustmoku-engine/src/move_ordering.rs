use rustmoku_core::{CELL_COUNT, Move, Stone};

use crate::{
    PatternState, bitboard::MOVES, line_geometry::CENTER_BIAS, move_generation::MoveList,
    pattern::ThreatProfile, search_heuristics::SearchHeuristics,
};

pub(crate) fn order_moves(
    side: Stone,
    patterns: &PatternState,
    moves: &mut MoveList,
    tt_move: Option<Move>,
    heuristics: &SearchHeuristics,
    ply: u8,
) {
    // Packed total order: tactical 56..64, TT 55, killer 53..55,
    // history 39..53, own 35..39, opponent 31..35, center 27..31,
    // reversed canonical index 0..8. Comparisons only read integers.
    let mut priorities = [0_u64; CELL_COUNT];
    let len = moves.as_slice().len();
    for (index, at) in moves.iter().enumerate() {
        let own = patterns.profile(at, side);
        let opponent = patterns.profile(at, side.opponent());
        priorities[index] = (u64::from(tactical_class(own, opponent)) << 56)
            | (u64::from(Some(at) == tt_move) << 55)
            | (u64::from(heuristics.killer_rank(ply, at)) << 53)
            | (u64::from(heuristics.history(side, at)) << 39)
            | ((own as u64) << 35)
            | ((opponent as u64) << 31)
            | (u64::from(CENTER_BIAS[at.index()]) << 27)
            | (CELL_COUNT - 1 - at.index()) as u64;
    }
    priorities[..len].sort_unstable_by(|left, right| right.cmp(left));
    for (at, &priority) in moves.as_mut_slice().iter_mut().zip(&priorities[..len]) {
        *at = MOVES[CELL_COUNT - 1 - (priority & 255) as usize];
    }
}

fn tactical_class(own: ThreatProfile, opponent: ThreatProfile) -> u8 {
    use ThreatProfile::{DoubleThree, FourThree, OpenThree, Three, WinningMove};
    // Four bits for tier, four for structural class. TT preference is confined
    // to the resulting class and can never displace a win or mandatory block.
    let (tier, profile) = if own == WinningMove {
        (9, own)
    } else if opponent == WinningMove {
        (8, opponent)
    } else if own >= FourThree {
        (7, own)
    } else if opponent >= FourThree {
        (6, opponent)
    } else if own >= DoubleThree {
        (5, own)
    } else if opponent >= DoubleThree {
        (4, opponent)
    } else if own >= OpenThree {
        (3, own)
    } else if opponent >= OpenThree {
        (2, opponent)
    } else if own == Three || opponent == Three {
        (1, own.max(opponent))
    } else {
        (0, own)
    };
    (tier << 4) | profile as u8
}

#[cfg(test)]
mod tests {
    use rustmoku_core::{Move, Position, Stone};

    use super::order_moves;
    use crate::search_heuristics::SearchHeuristics;
    use crate::{PatternState, move_generation::generate_candidates};

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
        order_moves(
            position.side_to_move(),
            &PatternState::new(&position),
            &mut moves,
            Some(tt_move),
            &SearchHeuristics::default(),
            0,
        );
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
        order_moves(
            position.side_to_move(),
            &PatternState::new(&position),
            &mut moves,
            Some(move_at(5, 5)),
            &SearchHeuristics::default(),
            0,
        );
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
        let patterns = PatternState::new(&position);
        let mut heuristics = SearchHeuristics::default();
        let quiet = move_at(5, 5);
        for _ in 0..1000 {
            heuristics.record_cutoff(position.side_to_move(), quiet, 20, 0, &patterns);
        }
        let block = move_at(7, 7);
        heuristics.record_cutoff(position.side_to_move(), block, 20, 0, &patterns);
        assert_eq!(heuristics.history(position.side_to_move(), block), 0);
        assert_eq!(heuristics.killer_rank(0, block), 0);
        order_moves(
            position.side_to_move(),
            &patterns,
            &mut moves,
            Some(move_at(5, 5)),
            &heuristics,
            0,
        );
        assert_eq!(moves.as_slice().first().copied(), Some(move_at(7, 7)));
    }

    #[test]
    fn packed_order_matches_lexicographic_reference_on_deterministic_boards() {
        use crate::line_geometry::CENTER_BIAS;
        let mut seed = 0x9e3779b97f4a7c15_u64;
        for _ in 0..32 {
            let mut position = Position::default();
            for _ in 0..90 {
                let legal: Vec<_> = Move::all().filter(|&at| position.is_legal(at)).collect();
                if legal.is_empty() {
                    break;
                }
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let at = legal[(seed % legal.len() as u64) as usize];
                position.make_move(at).unwrap();
            }
            let patterns = PatternState::new(&position);
            for side in [Stone::Black, Stone::White] {
                let mut moves = generate_candidates(&position);
                let tt_move = moves.as_slice().last().copied();
                let mut reference = moves.as_slice().to_vec();
                reference.sort_unstable_by_key(|&at| {
                    let own = patterns.profile(at, side);
                    let opponent = patterns.profile(at, side.opponent());
                    (
                        std::cmp::Reverse((
                            super::tactical_class(own, opponent),
                            Some(at) == tt_move,
                            own,
                            opponent,
                            CENTER_BIAS[at.index()],
                        )),
                        at,
                    )
                });
                order_moves(
                    side,
                    &patterns,
                    &mut moves,
                    tt_move,
                    &SearchHeuristics::default(),
                    0,
                );
                assert_eq!(moves.as_slice(), reference);
            }
        }
    }
}
