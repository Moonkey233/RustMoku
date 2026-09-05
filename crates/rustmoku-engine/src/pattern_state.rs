use rustmoku_core::{CELL_COUNT, Move, Position, RuleSet, Stone};

use crate::{
    bitboard::BitBoard256,
    line_geometry::{LINE_CELLS, LINE_INFLUENCES},
    pattern::{DirectionSet, LineKey, PatternPair, ThreatProfile, stone_index},
};

/// Opaque per-search pattern state, owned by PatternEvaluator's lifecycle.
/// Keys are maintained for occupied centers too; only empty profiles contribute.
#[derive(Debug, PartialEq, Eq)]
pub struct PatternState {
    occupied: BitBoard256,
    lines: [[LineKey; 4]; CELL_COUNT],
    directions: [DirectionSet; CELL_COUNT],
    profiles: [[ThreatProfile; 2]; CELL_COUNT],
    counts: [[u16; ThreatProfile::COUNT]; 2],
}

/// A bounded update is reversible from the played cell; no board snapshot.
#[derive(Debug)]
pub struct PatternUndo {
    at: Move,
    stone: Stone,
}

impl PatternState {
    pub(crate) fn new(position: &Position) -> Self {
        match position.rules() {
            RuleSet::Freestyle => {}
        }
        let mut state = Self {
            occupied: BitBoard256::EMPTY,
            lines: [[LineKey(0); 4]; CELL_COUNT],
            directions: [DirectionSet::default(); CELL_COUNT],
            profiles: [[ThreatProfile::Quiet; 2]; CELL_COUNT],
            counts: [[0; ThreatProfile::COUNT]; 2],
        };
        for at in Move::all() {
            if position.cell(at).is_some() {
                state.occupied.set(at);
            }
            for (direction, cells) in LINE_CELLS[at.index()].iter().enumerate() {
                for (field, &cell) in cells.iter().enumerate() {
                    let code = cell.map_or(3, |at| stone_code(position.cell(at)));
                    state.lines[at.index()][direction].0 |= code << (field * 2);
                }
            }
            state.directions[at.index()] =
                DirectionSet::new(state.lines[at.index()].map(PatternPair::lookup));
            if !state.occupied.test(at) {
                state.refresh_profile(at);
            }
        }
        state
    }

    pub(crate) fn profile(&self, at: Move, stone: Stone) -> ThreatProfile {
        self.profiles[at.index()][stone_index(stone)]
    }

    pub(crate) const fn counts(&self) -> &[[u16; ThreatProfile::COUNT]; 2] {
        &self.counts
    }

    pub(crate) fn make_move(&mut self, at: Move, stone: Stone) -> PatternUndo {
        debug_assert!(!self.occupied.test(at));
        self.remove_profile(at);
        self.profiles[at.index()] = [ThreatProfile::Quiet; 2];
        self.occupied.set(at);
        self.update_lines(at, 0, stone_code(Some(stone)));
        PatternUndo { at, stone }
    }

    pub(crate) fn unmake_move(&mut self, undo: PatternUndo) {
        debug_assert!(self.occupied.test(undo.at));
        self.update_lines(undo.at, stone_code(Some(undo.stone)), 0);
        self.occupied.clear(undo.at);
        self.refresh_profile(undo.at);
    }

    fn update_lines(&mut self, at: Move, old: u16, new: u16) {
        // A center lies on only one of these four axes (excluding the played
        // center itself), so each empty profile is removed/recomputed once.
        for influence in LINE_INFLUENCES[at.index()].iter() {
            let center = influence.center;
            let key = &mut self.lines[center.index()][usize::from(influence.direction)].0;
            debug_assert_eq!((*key >> influence.shift) & 3, old);
            *key = (*key & !(3 << influence.shift)) | (new << influence.shift);
            let pair = PatternPair::lookup(LineKey(*key));
            let changed = self.directions[center.index()].replace(influence.direction, pair);
            if changed && !self.occupied.test(center) {
                let profiles = self.directions[center.index()].profiles();
                if profiles != self.profiles[center.index()] {
                    self.remove_profile(center);
                    self.add_profile(center, profiles);
                }
            }
        }
    }

    fn remove_profile(&mut self, at: Move) {
        for (color, &profile) in self.profiles[at.index()].iter().enumerate() {
            self.counts[color][profile as usize] -= 1;
        }
    }

    fn refresh_profile(&mut self, at: Move) {
        self.add_profile(at, self.directions[at.index()].profiles());
    }

    fn add_profile(&mut self, at: Move, profiles: [ThreatProfile; 2]) {
        self.profiles[at.index()] = profiles;
        for (color, &profile) in profiles.iter().enumerate() {
            self.counts[color][profile as usize] += 1;
        }
    }
}

fn stone_code(stone: Option<Stone>) -> u16 {
    match stone {
        None => 0,
        Some(Stone::Black) => 1,
        Some(Stone::White) => 2,
    }
}

#[cfg(test)]
impl PatternState {
    /// Independent full geometry/encoding oracle, never present in Release.
    pub(crate) fn reference(position: &Position) -> Self {
        use crate::line_geometry::{DIRECTIONS, OFFSETS};
        let mut state = Self {
            occupied: BitBoard256::EMPTY,
            lines: [[LineKey(0); 4]; CELL_COUNT],
            directions: [DirectionSet::default(); CELL_COUNT],
            profiles: [[ThreatProfile::Quiet; 2]; CELL_COUNT],
            counts: [[0; ThreatProfile::COUNT]; 2],
        };
        for at in Move::all() {
            if position.cell(at).is_some() {
                state.occupied.set(at);
            }
            for (direction, (dr, dc)) in DIRECTIONS.into_iter().enumerate() {
                let mut key = 0;
                for (field, offset) in OFFSETS.into_iter().enumerate() {
                    let row = at.row() as isize + offset * dr;
                    let col = at.column() as isize + offset * dc;
                    let cell = usize::try_from(row)
                        .ok()
                        .zip(usize::try_from(col).ok())
                        .and_then(|(row, col)| Move::from_row_col(row, col).ok());
                    key |= cell.map_or(3, |cell| stone_code(position.cell(cell))) << (field * 2);
                }
                state.lines[at.index()][direction] = LineKey(key);
            }
            state.directions[at.index()] =
                DirectionSet::new(state.lines[at.index()].map(PatternPair::lookup));
            if position.cell(at).is_none() {
                for stone in [Stone::Black, Stone::White] {
                    let directions = state.lines[at.index()]
                        .map(|key| PatternPair::lookup(key).for_stone(stone));
                    let profile = ThreatProfile::from_directions(directions);
                    state.profiles[at.index()][stone_index(stone)] = profile;
                    state.counts[stone_index(stone)][profile as usize] += 1;
                }
            }
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Evaluator, PatternEvaluator};

    fn verify(position: &Position, state: &PatternState) {
        let reference = PatternState::reference(position);
        assert_eq!(state, &reference);
        assert_eq!(
            PatternEvaluator.evaluate(position, state),
            PatternEvaluator.evaluate(position, &reference)
        );
        for at in Move::all().filter(|&at| position.cell(at).is_none()) {
            for stone in [Stone::Black, Stone::White] {
                assert_eq!(
                    state.profile(at, stone) == ThreatProfile::WinningMove,
                    position.would_win(at, stone)
                );
            }
        }
        for counts in state.counts {
            assert_eq!(
                counts.iter().copied().sum::<u16>() as usize,
                CELL_COUNT - position.move_count()
            );
        }
    }

    #[test]
    fn deterministic_long_sequences_match_full_recompute_after_every_transition() {
        let mut seed = 0xa076_1d64_78bd_642f_u64;
        for _ in 0..32 {
            let mut position = Position::default();
            let mut state = PatternState::new(&position);
            let initial = PatternState::new(&position);
            let mut undos = Vec::new();
            verify(&position, &state);
            for _ in 0..180 {
                let legal: Vec<_> = Move::all().filter(|&at| position.is_legal(at)).collect();
                if legal.is_empty() {
                    break;
                }
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let at = legal[(seed % legal.len() as u64) as usize];
                let stone = position.side_to_move();
                undos.push((position.make_move(at).unwrap(), state.make_move(at, stone)));
                verify(&position, &state);
            }
            while let Some((position_undo, pattern_undo)) = undos.pop() {
                position.unmake_move(position_undo);
                state.unmake_move(pattern_undo);
                verify(&position, &state);
            }
            assert_eq!(state, initial);
        }
    }

    #[test]
    fn actual_positions_cover_compound_broken_and_all_directional_wins() {
        for (black, expected) in [
            (
                vec![(7, 4), (7, 5), (7, 8), (4, 7), (5, 7), (8, 7)],
                ThreatProfile::DoubleFour,
            ),
            (
                vec![(7, 4), (7, 5), (7, 8), (6, 7), (8, 7)],
                ThreatProfile::FourThree,
            ),
            (
                vec![(7, 6), (7, 8), (6, 7), (8, 7)],
                ThreatProfile::DoubleThree,
            ),
            (vec![(7, 4), (7, 5), (7, 8)], ThreatProfile::Four),
            (vec![(7, 5), (7, 9)], ThreatProfile::Three),
            (vec![(7, 5), (7, 8)], ThreatProfile::OpenThree),
        ] {
            let position = black_position(&black);
            let mut state = PatternState::new(&position);
            verify(&position, &state);
            assert_eq!(
                state.profile(Move::CENTER, Stone::Black),
                expected,
                "{black:?}"
            );
            let undo = state.make_move(Move::CENTER, Stone::Black);
            let mut played = position.clone();
            played.make_move(Move::CENTER).unwrap();
            verify(&played, &state);
            state.unmake_move(undo);
            verify(&position, &state);
        }
        for (dr, dc) in crate::line_geometry::DIRECTIONS {
            let black: Vec<_> = [-2, -1, 1, 2]
                .into_iter()
                .map(|offset| ((7 + dr * offset) as usize, (7 + dc * offset) as usize))
                .collect();
            let position = black_position(&black);
            let mut state = PatternState::new(&position);
            assert_eq!(
                state.profile(Move::CENTER, Stone::Black),
                ThreatProfile::WinningMove
            );
            let undo = state.make_move(Move::CENTER, Stone::Black);
            let mut won = position.clone();
            won.make_move(Move::CENTER).unwrap();
            verify(&won, &state);
            state.unmake_move(undo);
            verify(&position, &state);
        }
    }

    fn black_position(black: &[(usize, usize)]) -> Position {
        let mut position = Position::default();
        for (index, &(row, col)) in black.iter().enumerate() {
            position
                .make_move(Move::from_row_col(row, col).unwrap())
                .unwrap();
            position
                .make_move(Move::from_row_col(0, index * 2).unwrap())
                .unwrap();
        }
        position
    }
}
