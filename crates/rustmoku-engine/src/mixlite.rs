//! MixLite V3 scalar: bounded local updates, coarse/global nonlinear mixing,
//! cached WDL/value and context-conditioned candidate policy. No board CNN at leaves.
use crate::{
    Evaluator, LearnedModelError, PatternDelta, PatternState, bitboard::BitBoard256,
    learned::relative_key, line_geometry::LINE_INFLUENCES, pattern::stone_index,
};
use rustmoku_core::{CELL_COUNT, Move, Position, Stone};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path, sync::Arc};
const WIDTH: usize = 32;
const CONTEXT: usize = 8;
const FEATURES: usize = 65536;
const HEADER: usize = 32;
const MIX_INPUT: usize = 5 * WIDTH;
const BYTE_WEIGHTS: usize = (FEATURES + 3) * WIDTH + CONTEXT * MIX_INPUT;
const SHORT_WEIGHTS: usize = 3 * CONTEXT + WIDTH + CONTEXT;
const FILE_BYTES: usize = HEADER + BYTE_WEIGHTS + CONTEXT * 4 + SHORT_WEIGHTS * 2;
const GROUP_COUNTS: [i32; 4] = [64, 56, 56, 49];
fn group(at: Move) -> usize {
    usize::from(at.row() >= 8) * 2 + usize::from(at.column() >= 8)
}
fn activation(value: i32) -> i32 {
    value.clamp(0, 255)
}
#[derive(Debug)]
pub struct MixLiteModel {
    fingerprint: [u8; 32],
    embeddings: Box<[[i8; WIDTH]]>,
    centers: [[i8; WIDTH]; 3],
    mixing: [[i8; MIX_INPUT]; CONTEXT],
    bias: [i32; CONTEXT],
    wdl_head: [[i16; CONTEXT]; 3],
    policy_head: [i16; WIDTH],
    policy_context: [i16; CONTEXT],
    value_divisor: i32,
    policy_divisor: i32,
    score_scale: i32,
}
impl MixLiteModel {
    pub fn read_from_path(path: impl AsRef<Path>) -> Result<Self, LearnedModelError> {
        Self::read_from(&mut std::fs::File::open(path).map_err(LearnedModelError::Io)?)
    }
    pub fn read_from(reader: &mut impl Read) -> Result<Self, LearnedModelError> {
        let mut bytes = Vec::new();
        reader
            .take((FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(LearnedModelError::Io)?;
        if bytes.len() != FILE_BYTES
            || &bytes[..8] != b"RMLPV003"
            || bytes[8..20] != [3, 0, 3, 0, 0, 0, 1, 0, 32, 0, 2, 0]
        {
            return Err(LearnedModelError::Invalid(
                "invalid V3 header or tensor length",
            ));
        }
        let integer = |offset| {
            i32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("checked header"),
            )
        };
        let (value_divisor, policy_divisor, score_scale) = (integer(20), integer(24), integer(28));
        if value_divisor <= 0 || policy_divisor <= 0 || !(1..=10_000_000).contains(&score_scale) {
            return Err(LearnedModelError::Invalid("invalid V3 divisors/scale"));
        }
        let mut cursor = HEADER;
        let mut row = || {
            let values = std::array::from_fn(|i| bytes[cursor + i] as i8);
            cursor += WIDTH;
            values
        };
        let embeddings = (0..FEATURES)
            .map(|_| row())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let centers = std::array::from_fn(|_| row());
        let mixing = std::array::from_fn(|_| {
            let values = std::array::from_fn(|i| bytes[cursor + i] as i8);
            cursor += MIX_INPUT;
            values
        });
        let bias = std::array::from_fn(|_| {
            let value = integer(cursor);
            cursor += 4;
            value
        });
        if bias.iter().any(|value| value.unsigned_abs() > 1_048_576) {
            return Err(LearnedModelError::Invalid(
                "V3 bias exceeds accumulator bound",
            ));
        }
        let mut short = || {
            let value = i16::from_le_bytes(
                bytes[cursor..cursor + 2]
                    .try_into()
                    .expect("checked tensor length"),
            );
            cursor += 2;
            value
        };
        let wdl_head = std::array::from_fn(|_| std::array::from_fn(|_| short()));
        let policy_head = std::array::from_fn(|_| short());
        let policy_context = std::array::from_fn(|_| short());
        Ok(Self {
            fingerprint: Sha256::digest(&bytes).into(),
            embeddings,
            centers,
            mixing,
            bias,
            wdl_head,
            policy_head,
            policy_context,
            value_divisor,
            policy_divisor,
            score_scale,
        })
    }
    pub fn metadata(&self) -> crate::LearnedModelMetadata {
        crate::LearnedModelMetadata {
            format_version: 3,
            architecture_id: 3,
            feature_count: FEATURES,
            hidden: WIDTH,
            value_scale: self.value_divisor,
            policy_scale: self.policy_divisor,
        }
    }
    pub const fn score_scale(&self) -> i32 {
        self.score_scale
    }
    fn refresh(&self, state: &mut MixLiteState, side: usize) {
        let mut input = [0; MIX_INPUT];
        for lane in 0..WIDTH {
            let total: i32 = (0..4).map(|g| state.groups[side][g][lane]).sum();
            input[lane] = total / CELL_COUNT as i32;
            for g in 0..4 {
                input[(g + 1) * WIDTH + lane] = state.groups[side][g][lane] / GROUP_COUNTS[g];
            }
        }
        // 160 * 255 * 128 + bounded bias < 2^23; signed i32 is sufficient.
        for (lane, context) in state.context[side].iter_mut().enumerate() {
            let dot: i32 = input
                .iter()
                .zip(self.mixing[lane])
                .map(|(&a, b)| a * i32::from(b))
                .sum();
            *context = activation((dot + self.bias[lane]) / 256);
        }
        // WDL evidence is positive and bounded; no exp or lookup in inference.
        let evidence: [i64; 3] = self.wdl_head.map(|head| {
            let dot: i32 = head
                .into_iter()
                .zip(state.context[side])
                .map(|(a, b)| i32::from(a) * b)
                .sum();
            i64::from((dot / self.value_divisor).max(0)) + 1
        });
        let total: i64 = evidence.iter().sum();
        let win = evidence[0] * 32768 / total;
        let loss = evidence[2] * 32768 / total;
        state.wdl[side] = [win as u16, (32768 - win - loss) as u16, loss as u16];
        let q = (evidence[0] - evidence[2]) * 32768 / total;
        state.values[side] = (i64::from(self.score_scale) * q / (32768 - q.abs()))
            .clamp(-10_000_000, 10_000_000) as i32;
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct MixLiteState {
    preactivation: Box<[[[i32; WIDTH]; CELL_COUNT]; 2]>,
    groups: [[[i32; WIDTH]; 4]; 2],
    context: [[i32; CONTEXT]; 2],
    values: [i32; 2],
    wdl: [[u16; 3]; 2],
}
fn adjust_group(state: &mut MixLiteState, side: usize, at: Move, sign: i32) {
    let g = group(at);
    for lane in 0..WIDTH {
        state.groups[side][g][lane] +=
            sign * activation(state.preactivation[side][at.index()][lane]);
    }
}
#[derive(Clone, Debug)]
pub struct MixLiteEvaluator {
    model: Arc<MixLiteModel>,
}
impl MixLiteEvaluator {
    pub const fn new(model: Arc<MixLiteModel>) -> Self {
        Self { model }
    }
    pub fn model(&self) -> &Arc<MixLiteModel> {
        &self.model
    }
    pub fn evaluate_position(&self, position: &Position) -> i32 {
        let patterns = PatternState::new(position);
        self.evaluate(position, &patterns, &self.initialize(position, &patterns))
    }
    pub fn policy_for(&self, position: &Position, at: Move) -> Option<i32> {
        let patterns = PatternState::new(position);
        self.policy_score(
            position,
            &patterns,
            &self.initialize(position, &patterns),
            at,
        )
    }
    pub fn wdl_for(&self, position: &Position) -> [u16; 3] {
        let patterns = PatternState::new(position);
        self.initialize(position, &patterns).wdl[stone_index(position.side_to_move())]
    }
    fn apply(&self, state: &mut MixLiteState, delta: &PatternDelta, reverse: bool) {
        let at = delta.played_move();
        let mut dirty = BitBoard256::EMPTY;
        dirty.set(at);
        for influence in LINE_INFLUENCES[at.index()].iter() {
            dirty.set(influence.center);
        }
        for side in [Stone::Black, Stone::White] {
            let s = stone_index(side);
            for center in dirty.iter() {
                adjust_group(state, s, center, -1);
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
                adjust_group(state, s, center, 1);
            }
            self.model.refresh(state, s);
        }
    }
}
impl Evaluator for MixLiteEvaluator {
    fn supports_analysis_turn(&self) -> bool {
        true
    }
    fn model_fingerprint(&self) -> Option<[u8; 32]> {
        Some(self.model.fingerprint)
    }
    fn score_contract(&self) -> crate::ScoreContract {
        crate::ScoreContract::RationalV2 {
            scale: self.model.score_scale,
        }
    }
    type State = MixLiteState;
    type Undo = ();
    fn initialize(&self, position: &Position, patterns: &PatternState) -> Self::State {
        let mut state = MixLiteState {
            preactivation: Box::new([[[0; WIDTH]; CELL_COUNT]; 2]),
            groups: [[[0; WIDTH]; 4]; 2],
            context: [[0; CONTEXT]; 2],
            values: [0; 2],
            wdl: [[0; 3]; 2],
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
                adjust_group(&mut state, s, at, 1);
            }
            self.model.refresh(&mut state, s);
        }
        state
    }

    fn make_move(&self, state: &mut MixLiteState, delta: &PatternDelta) {
        self.apply(state, delta, false);
    }
    fn unmake_move(&self, state: &mut MixLiteState, delta: &PatternDelta, _: ()) {
        self.apply(state, delta, true);
    }
    fn evaluate(&self, position: &Position, _: &PatternState, state: &MixLiteState) -> i32 {
        state.values[stone_index(position.side_to_move())]
    }
    fn policy_score(
        &self,
        position: &Position,
        _: &PatternState,
        state: &MixLiteState,
        at: Move,
    ) -> Option<i32> {
        position.is_legal(at).then(|| {
            let side = stone_index(position.side_to_move());
            let local = &state.preactivation[side][at.index()];
            let dot: i64 = local
                .iter()
                .zip(self.model.policy_head)
                .map(|(&a, b)| i64::from(activation(a)) * i64::from(b))
                .sum();
            // Bilinear local/context interaction needs i64 (up to 2^35).
            let cross: i64 = (0..CONTEXT)
                .map(|i| {
                    i64::from(activation(local[i]))
                        * i64::from(state.context[side][i])
                        * i64::from(self.model.policy_context[i])
                })
                .sum();
            ((dot + cross / 256) / i64::from(self.model.policy_divisor)).clamp(-32768, 32767) as i32
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (MixLiteEvaluator, Vec<u8>) {
        let mut bytes = vec![0; FILE_BYTES];
        bytes[..8].copy_from_slice(b"RMLPV003");
        bytes[8..20].copy_from_slice(&[3, 0, 3, 0, 0, 0, 1, 0, 32, 0, 2, 0]);
        for (at, value) in [(20, 16i32), (24, 64), (28, 500)] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (i, byte) in bytes[HEADER..HEADER + BYTE_WEIGHTS].iter_mut().enumerate() {
            *byte = ((i * 31 % 127) as i8 - 63) as u8;
        }
        let bias = HEADER + BYTE_WEIGHTS;
        for i in 0..CONTEXT {
            bytes[bias + i * 4..bias + i * 4 + 4]
                .copy_from_slice(&(512i32 * (i as i32 + 1)).to_le_bytes());
        }
        for (i, pair) in bytes[bias + CONTEXT * 4..]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .enumerate()
        {
            pair.copy_from_slice(&((i as i16 * 83) - 2000).to_le_bytes());
        }
        (
            MixLiteEvaluator::new(Arc::new(MixLiteModel::read_from(&mut &bytes[..]).unwrap())),
            bytes,
        )
    }
    #[test]
    fn v3_incremental_scalar_matches_rebuild_and_null_restores() {
        let (evaluator, _) = fixture();
        let original = Position::default();
        let mut state = crate::search_state::SearchState::new(&original, &evaluator);
        let mut undos = Vec::new();
        for i in 0..45 {
            let at = Move::from_index((i * 47 + 112) % 225).unwrap();
            if state.position().winner().is_some() {
                break;
            }
            undos.push(state.make_move(at, &evaluator).unwrap());
            state.assert_consistent(&evaluator);
            assert!(state.evaluate(&evaluator).abs() <= 10_000_000);
            if let Some(turn) = state.begin_null(&evaluator) {
                state.assert_consistent(&evaluator);
                state.end_null(turn);
                state.assert_consistent(&evaluator);
            }
        }
        while let Some(undo) = undos.pop() {
            state.unmake_move(undo, &evaluator);
            state.assert_consistent(&evaluator);
        }
        assert_eq!(state.position(), &original);
        assert_eq!(
            evaluator
                .wdl_for(&original)
                .into_iter()
                .map(u32::from)
                .sum::<u32>(),
            32768
        );
    }
    #[test]
    fn v3_extreme_heads_stay_bounded_outside_mate_range() {
        let (_, original) = fixture();
        for negative in [false, true] {
            let mut bytes = original.clone();
            bytes[HEADER..HEADER + BYTE_WEIGHTS].fill(127);
            for offset in [20, 24] {
                bytes[offset..offset + 4].copy_from_slice(&1i32.to_le_bytes());
            }
            bytes[28..32].copy_from_slice(&10_000_000i32.to_le_bytes());
            let bias = HEADER + BYTE_WEIGHTS;
            for pair in bytes[bias..bias + CONTEXT * 4].as_chunks_mut::<4>().0 {
                *pair = 1_048_576i32.to_le_bytes();
            }
            let heads = bias + CONTEXT * 4;
            for (i, pair) in bytes[heads..].as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let positive = if i < 3 * CONTEXT {
                    i / CONTEXT == if negative { 2 } else { 0 }
                } else {
                    !negative
                };
                *pair = if positive { i16::MAX } else { i16::MIN }.to_le_bytes();
            }
            let evaluator =
                MixLiteEvaluator::new(Arc::new(MixLiteModel::read_from(&mut &bytes[..]).unwrap()));
            let position = Position::default();
            assert_eq!(
                evaluator.evaluate_position(&position),
                if negative { -10_000_000 } else { 10_000_000 }
            );
            assert_eq!(
                evaluator.policy_for(&position, Move::CENTER),
                Some(if negative { -32768 } else { 32767 })
            );
        }
    }

    #[test]
    fn v3_parser_rejects_overflow_bias_truncation_and_extra_bytes() {
        let (_, bytes) = fixture();
        assert!(MixLiteModel::read_from(&mut &bytes[..bytes.len() - 1]).is_err());
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(MixLiteModel::read_from(&mut &extra[..]).is_err());
        let mut invalid = bytes;
        let at = HEADER + BYTE_WEIGHTS;
        invalid[at..at + 4].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(MixLiteModel::read_from(&mut &invalid[..]).is_err());
    }
}
