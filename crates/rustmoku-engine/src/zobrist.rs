use rustmoku_core::{CELL_COUNT, Move, Position, RuleSet, Stone};

const ZOBRIST_SEED: u64 = 0x5255_5354_4D4F_4B55;
const SPLITMIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const PIECE_KEY_COUNT: usize = 2 * CELL_COUNT;

const PIECE_KEYS: [[u64; CELL_COUNT]; 2] = generate_piece_keys();
const BLACK_TO_MOVE_KEY: u64 = generated_key(PIECE_KEY_COUNT);
const WHITE_TO_MOVE_KEY: u64 = generated_key(PIECE_KEY_COUNT + 1);
const FREESTYLE_RULE_KEY: u64 = generated_key(PIECE_KEY_COUNT + 2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PositionKey(u64);

impl PositionKey {
    pub(crate) fn from_position(position: &Position) -> Self {
        let mut key = side_key(position.side_to_move()) ^ rule_key(position.rules());
        for at in Move::all() {
            if let Some(stone) = position.cell(at) {
                key ^= piece_key(stone, at);
            }
        }
        Self(key)
    }

    pub(crate) const fn value(self) -> u64 {
        self.0
    }

    pub(crate) fn toggle_move(self, at: Move, stone: Stone) -> Self {
        Self(self.0 ^ piece_key(stone, at) ^ side_key(stone) ^ side_key(stone.opponent()))
    }
}

const fn generate_piece_keys() -> [[u64; CELL_COUNT]; 2] {
    let mut keys = [[0; CELL_COUNT]; 2];
    let mut stone = 0;
    while stone < 2 {
        let mut index = 0;
        while index < CELL_COUNT {
            keys[stone][index] = generated_key(stone * CELL_COUNT + index);
            index += 1;
        }
        stone += 1;
    }
    keys
}

const fn generated_key(index: usize) -> u64 {
    splitmix64(ZOBRIST_SEED.wrapping_add((index as u64).wrapping_mul(SPLITMIX_GAMMA)))
}

const fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(SPLITMIX_GAMMA);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

const fn stone_index(stone: Stone) -> usize {
    match stone {
        Stone::Black => 0,
        Stone::White => 1,
    }
}

const fn piece_key(stone: Stone, at: Move) -> u64 {
    PIECE_KEYS[stone_index(stone)][at.index()]
}

const fn side_key(side_to_move: Stone) -> u64 {
    match side_to_move {
        Stone::Black => BLACK_TO_MOVE_KEY,
        Stone::White => WHITE_TO_MOVE_KEY,
    }
}

const fn rule_key(rules: RuleSet) -> u64 {
    match rules {
        RuleSet::Freestyle => FREESTYLE_RULE_KEY,
    }
}

#[cfg(test)]
mod tests {
    use rustmoku_core::{Move, Position, RuleSet, Stone};

    use super::{
        FREESTYLE_RULE_KEY, PIECE_KEY_COUNT, PositionKey, generated_key, piece_key, rule_key,
        side_key,
    };

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
    fn independently_reconstructed_positions_have_identical_keys() {
        let moves = [(7, 7), (6, 7), (8, 8), (7, 8)];
        assert_eq!(
            PositionKey::from_position(&position_from(&moves)),
            PositionKey::from_position(&position_from(&moves))
        );
    }

    #[test]
    fn move_order_transpositions_have_identical_keys() {
        let first = position_from(&[(7, 7), (6, 7), (8, 8), (6, 8)]);
        let second = position_from(&[(8, 8), (6, 8), (7, 7), (6, 7)]);
        assert_eq!(
            PositionKey::from_position(&first),
            PositionKey::from_position(&second)
        );
    }

    #[test]
    fn distinct_test_positions_have_distinct_keys() {
        let first = position_from(&[(7, 7), (6, 7)]);
        let second = position_from(&[(7, 8), (6, 7)]);
        assert_ne!(
            PositionKey::from_position(&first),
            PositionKey::from_position(&second)
        );
    }

    #[test]
    fn incremental_move_and_unmake_match_full_recomputation() {
        let mut position = position_from(&[(7, 7), (6, 7)]);
        let original_position = position.clone();
        let original_key = PositionKey::from_position(&position);
        let at = move_at(8, 8);
        let stone = position.side_to_move();

        let undo = position.make_move(at).expect("test move must be legal");
        let moved_key = original_key.toggle_move(at, stone);
        assert_eq!(moved_key, PositionKey::from_position(&position));

        position.unmake_move(undo);
        let restored_key = moved_key.toggle_move(at, stone);
        assert_eq!(restored_key, original_key);
        assert_eq!(position, original_position);
    }

    #[test]
    fn side_and_rule_components_are_explicit_and_deterministic() {
        assert_ne!(side_key(Stone::Black), side_key(Stone::White));
        assert_eq!(rule_key(RuleSet::Freestyle), FREESTYLE_RULE_KEY);
        assert_ne!(FREESTYLE_RULE_KEY, 0);
        assert_eq!(
            PositionKey::from_position(&Position::default()).value(),
            side_key(Stone::Black) ^ FREESTYLE_RULE_KEY
        );

        let mut after_black_move = Position::default();
        after_black_move
            .make_move(Move::CENTER)
            .expect("center must be legal");
        assert_eq!(
            PositionKey::from_position(&after_black_move).value(),
            side_key(Stone::White) ^ FREESTYLE_RULE_KEY ^ piece_key(Stone::Black, Move::CENTER)
        );
    }

    #[test]
    fn generated_table_has_no_zero_or_duplicate_keys() {
        let key_count = PIECE_KEY_COUNT + 3;
        for left in 0..key_count {
            let left_key = generated_key(left);
            assert_ne!(left_key, 0, "key {left} unexpectedly generated zero");
            for right in (left + 1)..key_count {
                assert_ne!(
                    left_key,
                    generated_key(right),
                    "keys {left} and {right} collide"
                );
            }
        }
    }
}
