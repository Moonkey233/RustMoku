//! Width-8 integer ReLU evaluator. Models are immutable; accumulators belong to
//! one search worker. The score contract is rational STM normalization with a
//! frozen calibration scale; network outputs can never enter the mate range.

use sha2::{Digest, Sha256};
use std::{io::Read, path::Path, sync::Arc};

use rustmoku_core::{CELL_COUNT, Move, Position, Stone};

use crate::{
    Evaluator, LearnedModelError, PatternDelta, PatternState, bitboard::BitBoard256,
    evaluation::EVALUATION_LIMIT, learned::relative_key, line_geometry::LINE_INFLUENCES,
    pattern::stone_index,
};

pub const NONLINEAR_WIDTH: usize = 8;
const WIDTH: usize = NONLINEAR_WIDTH;
const FEATURES: usize = 65536;
const HEADER: usize = 44;
const WEIGHTS: usize = (FEATURES + 3 + 2) * WIDTH;
const FILE_BYTES: usize = HEADER + WEIGHTS * 2;
const UNIT: i64 = 32768;
const CLIP: i64 = 32735;
// A center contains five i16 embeddings. ReLU cannot increase absolute value.
// preactivation <= 5*32768 fits i32; every possible legal model head sum fits
// i64. Updates subtract/add one center without including the bias.
const MAX_HEAD_SUM: i64 = CELL_COUNT as i64 * WIDTH as i64 * 5 * 32768 * 32768;

#[derive(Debug)]
pub struct NonlinearModel {
    fingerprint: [u8; 32],
    embeddings: Box<[[i16; WIDTH]]>,
    centers: [[i16; WIDTH]; 3],
    value_head: [i16; WIDTH],
    policy_head: [i16; WIDTH],
    bias: i64,
    value_divisor: i32,
    policy_divisor: i32,
    score_scale: i32,
}

impl NonlinearModel {
    pub fn read_from_path(path: impl AsRef<Path>) -> Result<Self, LearnedModelError> {
        let mut file = std::fs::File::open(path).map_err(LearnedModelError::Io)?;
        Self::read_from(&mut file)
    }

    pub fn read_from(reader: &mut impl Read) -> Result<Self, LearnedModelError> {
        let mut bytes = Vec::new();
        reader
            .take((FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(LearnedModelError::Io)?;
        if bytes.len() != FILE_BYTES || &bytes[..8] != b"RMLPV002" {
            return Err(LearnedModelError::Invalid(
                "invalid V2 model magic or length",
            ));
        }
        // <8sHHIHHiiiqI>: format, architecture, features, width, contract,
        // value divisor, policy divisor, frozen score scale, raw bias, flags.
        if bytes[8..20] != [2, 0, 2, 0, 0, 0, 1, 0, 8, 0, 2, 0] || bytes[40..44] != [0; 4] {
            return Err(LearnedModelError::Invalid(
                "unsupported V2 architecture or score contract",
            ));
        }
        let integer = |offset| {
            i32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("checked header"),
            )
        };
        let value_divisor = integer(20);
        let policy_divisor = integer(24);
        let score_scale = integer(28);
        let bias = i64::from_le_bytes(bytes[32..40].try_into().expect("checked header"));
        if value_divisor <= 0
            || policy_divisor <= 0
            || !(1..=EVALUATION_LIMIT).contains(&score_scale)
            || bias.unsigned_abs() > (i64::MAX - MAX_HEAD_SUM) as u64
        {
            return Err(LearnedModelError::Invalid(
                "V2 scale or bias violates arithmetic contract",
            ));
        }
        let mut values = bytes[HEADER..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| i16::from_le_bytes([v[0], v[1]]));
        let mut row =
            || std::array::from_fn(|_| values.next().expect("checked payload dimensions"));
        let embeddings = (0..FEATURES).map(|_| row()).collect();
        let centers = std::array::from_fn(|_| row());
        let value_head = row();
        let policy_head = row();
        Ok(Self {
            fingerprint: Sha256::digest(&bytes).into(),
            embeddings,
            centers,
            value_head,
            policy_head,
            bias,
            value_divisor,
            policy_divisor,
            score_scale,
        })
    }

    #[must_use]
    pub const fn score_scale(&self) -> i32 {
        self.score_scale
    }

    #[must_use]
    pub fn metadata(&self) -> crate::LearnedModelMetadata {
        crate::LearnedModelMetadata {
            format_version: 2,
            architecture_id: 2,
            feature_count: FEATURES,
            hidden: WIDTH,
            value_scale: self.value_divisor,
            policy_scale: self.policy_divisor,
        }
    }

    fn normalized(&self, sum: i64) -> i64 {
        // Aggregate before the single division, including the bias in the same
        // numerator. Rust signed integer division truncates toward zero.
        ((sum + self.bias) / (CELL_COUNT as i64 * i64::from(self.value_divisor))).clamp(-CLIP, CLIP)
    }

    fn value(&self, sum: i64) -> i32 {
        let q = self.normalized(sum);
        let score = i64::from(self.score_scale) * q / (UNIT - q.abs());
        score.clamp(-i64::from(EVALUATION_LIMIT), i64::from(EVALUATION_LIMIT)) as i32
    }
}

#[derive(Clone, Debug)]
pub struct NonlinearEvaluator {
    model: Arc<NonlinearModel>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct NonlinearState {
    preactivation: Box<[[[i32; WIDTH]; CELL_COUNT]; 2]>,
    value_sums: [i64; 2],
}

fn dot(preactivation: &[i32; WIDTH], head: &[i16; WIDTH]) -> i64 {
    preactivation
        .iter()
        .zip(head)
        .map(|(&a, &b)| i64::from(a.max(0)) * i64::from(b))
        .sum()
}

impl NonlinearEvaluator {
    #[must_use]
    pub const fn new(model: Arc<NonlinearModel>) -> Self {
        Self { model }
    }

    #[must_use]
    pub fn model(&self) -> &Arc<NonlinearModel> {
        &self.model
    }

    #[must_use]
    pub fn evaluate_position(&self, position: &Position) -> i32 {
        let patterns = PatternState::new(position);
        let state = self.initialize(position, &patterns);
        self.evaluate(position, &patterns, &state)
    }

    #[must_use]
    pub fn policy_for(&self, position: &Position, at: Move) -> Option<i32> {
        let patterns = PatternState::new(position);
        let state = self.initialize(position, &patterns);
        self.policy_score(position, &patterns, &state, at)
    }

    fn apply(&self, state: &mut NonlinearState, delta: &PatternDelta, reverse: bool) {
        let at = delta.played_move();
        let mut dirty = BitBoard256::EMPTY;
        dirty.set(at);
        for influence in LINE_INFLUENCES[at.index()].iter() {
            dirty.set(influence.center);
        }
        for side in [Stone::Black, Stone::White] {
            let s = stone_index(side);
            for center in dirty.iter() {
                state.value_sums[s] -= dot(
                    &state.preactivation[s][center.index()],
                    &self.model.value_head,
                );
            }
            for (influence, (old, new)) in LINE_INFLUENCES[at.index()].iter().zip(delta.changes()) {
                let (old, new) = if reverse { (new, old) } else { (old, new) };
                let old = self.model.embeddings[usize::from(relative_key(old, side).0)];
                let new = self.model.embeddings[usize::from(relative_key(new, side).0)];
                for (d, sum) in state.preactivation[s][influence.center.index()]
                    .iter_mut()
                    .enumerate()
                {
                    *sum += i32::from(new[d]) - i32::from(old[d]);
                }
            }
            let occupied = if delta.played_stone() == side { 1 } else { 2 };
            let (old, new) = if reverse {
                (occupied, 0)
            } else {
                (0, occupied)
            };
            for (d, sum) in state.preactivation[s][at.index()].iter_mut().enumerate() {
                *sum +=
                    i32::from(self.model.centers[new][d]) - i32::from(self.model.centers[old][d]);
            }
            for center in dirty.iter() {
                state.value_sums[s] += dot(
                    &state.preactivation[s][center.index()],
                    &self.model.value_head,
                );
            }
        }
    }
}

impl Evaluator for NonlinearEvaluator {
    fn model_fingerprint(&self) -> Option<[u8; 32]> {
        Some(self.model.fingerprint)
    }
    fn score_contract(&self) -> crate::ScoreContract {
        crate::ScoreContract::RationalV2 {
            scale: self.model.score_scale,
        }
    }
    type State = NonlinearState;
    type Undo = ();

    fn initialize(&self, position: &Position, patterns: &PatternState) -> Self::State {
        let mut state = NonlinearState {
            preactivation: Box::new([[[0; WIDTH]; CELL_COUNT]; 2]),
            value_sums: [0; 2],
        };
        for side in [Stone::Black, Stone::White] {
            let s = stone_index(side);
            for at in Move::all() {
                let center = position
                    .cell(at)
                    .map_or(0, |stone| if stone == side { 1 } else { 2 });
                let mut local = self.model.centers[center].map(i32::from);
                for key in patterns.line_keys(at) {
                    for (sum, weight) in local
                        .iter_mut()
                        .zip(self.model.embeddings[usize::from(relative_key(key, side).0)])
                    {
                        *sum += i32::from(weight);
                    }
                }
                state.preactivation[s][at.index()] = local;
                state.value_sums[s] += dot(&local, &self.model.value_head);
            }
        }
        state
    }

    fn make_move(&self, state: &mut Self::State, delta: &PatternDelta) {
        self.apply(state, delta, false);
    }
    fn unmake_move(&self, state: &mut Self::State, delta: &PatternDelta, _undo: ()) {
        self.apply(state, delta, true);
    }
    fn evaluate(&self, position: &Position, _patterns: &PatternState, state: &Self::State) -> i32 {
        self.model
            .value(state.value_sums[stone_index(position.side_to_move())])
    }
    fn policy_score(
        &self,
        position: &Position,
        _patterns: &PatternState,
        state: &Self::State,
        at: Move,
    ) -> Option<i32> {
        position.is_legal(at).then(|| {
            let sum = dot(
                &state.preactivation[stone_index(position.side_to_move())][at.index()],
                &self.model.policy_head,
            );
            (sum / i64::from(self.model.policy_divisor))
                .clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i32
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AlphaBetaEngine, EngineConfig, SearchEngine, SearchLimits, search_state::SearchState,
    };

    pub(super) fn fixture(extreme: bool) -> NonlinearEvaluator {
        let model = NonlinearModel {
            fingerprint: [0; 32],
            embeddings: (0..FEATURES)
                .map(|key| {
                    std::array::from_fn(|d| {
                        if extreme {
                            if (key + d) % 2 == 0 {
                                i16::MIN
                            } else {
                                i16::MAX
                            }
                        } else {
                            ((key * 17 + d * 13) % 127) as i16 - 63
                        }
                    })
                })
                .collect(),
            centers: if extreme {
                [[i16::MIN; WIDTH], [i16::MAX; WIDTH], [i16::MIN; WIDTH]]
            } else {
                [[-100; WIDTH], [200; WIDTH], [-300; WIDTH]]
            },
            value_head: std::array::from_fn(|d| if d % 2 == 0 { i16::MIN } else { i16::MAX }),
            policy_head: [i16::MIN; WIDTH],
            bias: -1234567,
            value_divisor: 2048,
            policy_divisor: 17,
            score_scale: 1000,
        };
        NonlinearEvaluator::new(Arc::new(model))
    }

    #[test]
    fn nonlinear_incremental_center_relu_and_extremes_restore() {
        for extreme in [false, true] {
            let evaluator = fixture(extreme);
            let mut state = SearchState::new(&Position::default(), &evaluator);
            let mut undos = Vec::new();
            for index in (0..CELL_COUNT).map(|i| i * 97 % CELL_COUNT) {
                let at = Move::from_index(index).unwrap();
                if !state.position().is_legal(at) {
                    break;
                }
                undos.push(state.make_move(at, &evaluator).unwrap());
                state.assert_consistent(&evaluator);
                assert!(state.make_move(at, &evaluator).is_err());
                state.assert_consistent(&evaluator);
            }
            while let Some(undo) = undos.pop() {
                state.unmake_move(undo, &evaluator);
                state.assert_consistent(&evaluator);
            }
            assert_eq!(state.position(), &Position::default());
        }
    }

    #[test]
    fn nonlinear_full_draw_and_extreme_bias_round_trip() {
        let mut evaluator = fixture(true);
        Arc::get_mut(&mut evaluator.model).unwrap().bias = i64::MAX - MAX_HEAD_SUM;
        let mut state = SearchState::new(&Position::default(), &evaluator);
        let mut black = Move::all().filter(|at| (at.row() + 2 * at.column()) % 4 < 2);
        let mut white = Move::all().filter(|at| (at.row() + 2 * at.column()) % 4 >= 2);
        let mut undos = Vec::new();
        for ply in 0..CELL_COUNT {
            let at = if ply % 2 == 0 {
                black.next().unwrap()
            } else {
                white.next().unwrap()
            };
            undos.push(state.make_move(at, &evaluator).unwrap());
            state.assert_consistent(&evaluator);
            assert!(state.evaluate(&evaluator).abs() <= EVALUATION_LIMIT);
        }
        assert!(state.position().is_full());
        assert_eq!(state.position().winner(), None);
        assert!(Move::all().all(|at| evaluator.policy_for(state.position(), at).is_none()));
        while let Some(undo) = undos.pop() {
            state.unmake_move(undo, &evaluator);
            state.assert_consistent(&evaluator);
        }
        Arc::get_mut(&mut evaluator.model).unwrap().bias = -i64::MAX + MAX_HEAD_SUM;
        assert!(evaluator.evaluate_position(&Position::default()).abs() <= EVALUATION_LIMIT);
    }

    #[test]
    fn nonlinear_search_stop_and_smp_preserve_caller() {
        let evaluator = fixture(true);
        let position = Position::default();
        for threads in [1, 2] {
            let mut engine = AlphaBetaEngine::with_config(
                evaluator.clone(),
                EngineConfig::default().with_threads(threads),
            );
            let result = engine.search(&position, SearchLimits::new(4).with_max_nodes(100));
            assert!(result.best_move.is_some());
            assert_eq!(position, Position::default());
        }
    }
}
