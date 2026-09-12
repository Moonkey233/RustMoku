//! Quantized scalar local-pattern Value/Policy inference.
use std::{
    fmt,
    fs::File,
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
};

use rustmoku_core::{CELL_COUNT, Move, Position, Stone};
use sha2::{Digest, Sha256};

use crate::{
    Evaluator, PatternDelta, PatternState,
    evaluation::EVALUATION_LIMIT,
    pattern::{LineKey, stone_index},
};

const MAGIC: &[u8; 8] = b"RMLPV001";
const FORMAT_VERSION: u16 = 1;
const ARCHITECTURE_ID: u16 = 1;
pub const LEARNED_FEATURE_COUNT: usize = 1 << 16;
pub const LEARNED_HIDDEN: usize = 16;
const MAX_MODEL_BYTES: u64 = 4 * 1024 * 1024;
const FEATURES_PER_POSITION: i64 = (CELL_COUNT * 4) as i64;
const I16_ABS_MAX: i64 = 1_i64 << 15;
const MAX_ACCUMULATOR: i64 = FEATURES_PER_POSITION * I16_ABS_MAX;
const MAX_VALUE_DOT: i64 = MAX_ACCUMULATOR * I16_ABS_MAX * LEARNED_HIDDEN as i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LearnedModelMetadata {
    pub format_version: u16,
    pub architecture_id: u16,
    pub feature_count: usize,
    pub hidden: usize,
    pub value_scale: i32,
    pub policy_scale: i32,
}

/// Immutable weights shared by all search workers.
#[derive(Debug)]
pub struct LearnedModel {
    fingerprint: [u8; 32],
    embeddings: Box<[i16]>,
    value_head: [i16; LEARNED_HIDDEN],
    policy_head: [i16; LEARNED_HIDDEN],
    value_table: Box<[i64]>,
    policy_table: Box<[i64]>,
    value_bias: i64,
    value_scale: i32,
    policy_scale: i32,
}

impl LearnedModel {
    pub fn read_from_path(path: impl AsRef<Path>) -> Result<Self, LearnedModelError> {
        let path = path.as_ref();
        let length = std::fs::metadata(path)
            .map_err(LearnedModelError::Io)?
            .len();
        if length > MAX_MODEL_BYTES {
            return Err(LearnedModelError::Invalid("model file exceeds size limit"));
        }
        let mut file = File::open(path).map_err(LearnedModelError::Io)?;
        Self::read_from(&mut file)
    }

    pub fn read_from(reader: &mut impl Read) -> Result<Self, LearnedModelError> {
        let mut bytes = Vec::new();
        reader
            .take(MAX_MODEL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(LearnedModelError::Io)?;
        if bytes.len() as u64 > MAX_MODEL_BYTES {
            return Err(LearnedModelError::Invalid("model file exceeds size limit"));
        }
        let mut decoder = Decoder::new(&bytes);
        if decoder.bytes(8)? != MAGIC {
            return Err(LearnedModelError::Invalid("invalid model magic"));
        }
        if decoder.u16()? != FORMAT_VERSION {
            return Err(LearnedModelError::Invalid(
                "unsupported model format version",
            ));
        }
        if decoder.u16()? != ARCHITECTURE_ID {
            return Err(LearnedModelError::Invalid("unsupported model architecture"));
        }
        let feature_count = usize::try_from(decoder.u32()?)
            .map_err(|_| LearnedModelError::Invalid("feature count overflow"))?;
        let hidden = usize::from(decoder.u16()?);
        if decoder.u16()? != 0 {
            return Err(LearnedModelError::Invalid("nonzero model flags"));
        }
        let value_scale = decoder.i32()?;
        let value_bias = decoder.i64()?;
        let policy_scale = decoder.i32()?;
        let embedding_count = usize::try_from(decoder.u32()?)
            .map_err(|_| LearnedModelError::Invalid("embedding count overflow"))?;
        let value_count = usize::from(decoder.u16()?);
        let policy_count = usize::from(decoder.u16()?);
        if feature_count != LEARNED_FEATURE_COUNT
            || hidden != LEARNED_HIDDEN
            || embedding_count != LEARNED_FEATURE_COUNT * LEARNED_HIDDEN
            || value_count != LEARNED_HIDDEN
            || policy_count != LEARNED_HIDDEN
        {
            return Err(LearnedModelError::Invalid(
                "model dimensions do not match architecture",
            ));
        }
        if value_scale <= 0 || policy_scale <= 0 {
            return Err(LearnedModelError::Invalid(
                "quantization scales must be positive",
            ));
        }
        if value_bias.unsigned_abs() > (i64::MAX - MAX_VALUE_DOT) as u64 {
            return Err(LearnedModelError::Invalid(
                "value bias violates arithmetic bounds",
            ));
        }
        let expected_values = embedding_count
            .checked_add(value_count)
            .and_then(|count| count.checked_add(policy_count))
            .ok_or(LearnedModelError::Invalid("model length overflow"))?;
        let expected_bytes = expected_values
            .checked_mul(2)
            .ok_or(LearnedModelError::Invalid("model byte length overflow"))?;
        if decoder.remaining() != expected_bytes {
            return Err(LearnedModelError::Invalid("model payload length mismatch"));
        }
        let mut embeddings = Vec::with_capacity(embedding_count);
        for _ in 0..embedding_count {
            embeddings.push(decoder.i16()?);
        }
        let mut value_head = [0; LEARNED_HIDDEN];
        for weight in &mut value_head {
            *weight = decoder.i16()?;
        }
        let mut policy_head = [0; LEARNED_HIDDEN];
        for weight in &mut policy_head {
            *weight = decoder.i16()?;
        }
        debug_assert_eq!(decoder.remaining(), 0);
        let (value_table, policy_table) = compile_tables(&embeddings, &value_head, &policy_head);
        Ok(Self {
            fingerprint: Sha256::digest(&bytes).into(),
            value_table,
            policy_table,
            embeddings: embeddings.into_boxed_slice(),
            value_head,
            policy_head,
            value_bias,
            value_scale,
            policy_scale,
        })
    }

    pub fn write_to_path(&self, path: impl AsRef<Path>) -> Result<(), LearnedModelError> {
        let mut file = File::create(path).map_err(LearnedModelError::Io)?;
        self.write_to(&mut file)
    }

    pub fn write_to(&self, writer: &mut impl Write) -> Result<(), LearnedModelError> {
        writer.write_all(MAGIC).map_err(LearnedModelError::Io)?;
        for bytes in [FORMAT_VERSION.to_le_bytes(), ARCHITECTURE_ID.to_le_bytes()] {
            writer.write_all(&bytes).map_err(LearnedModelError::Io)?;
        }
        writer
            .write_all(&(LEARNED_FEATURE_COUNT as u32).to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        writer
            .write_all(&(LEARNED_HIDDEN as u16).to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        writer
            .write_all(&0_u16.to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        writer
            .write_all(&self.value_scale.to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        writer
            .write_all(&self.value_bias.to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        writer
            .write_all(&self.policy_scale.to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        writer
            .write_all(&(self.embeddings.len() as u32).to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        writer
            .write_all(&(LEARNED_HIDDEN as u16).to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        writer
            .write_all(&(LEARNED_HIDDEN as u16).to_le_bytes())
            .map_err(LearnedModelError::Io)?;
        for value in self
            .embeddings
            .iter()
            .chain(&self.value_head)
            .chain(&self.policy_head)
        {
            writer
                .write_all(&value.to_le_bytes())
                .map_err(LearnedModelError::Io)?;
        }
        Ok(())
    }

    #[must_use]
    pub const fn metadata(&self) -> LearnedModelMetadata {
        LearnedModelMetadata {
            format_version: FORMAT_VERSION,
            architecture_id: ARCHITECTURE_ID,
            feature_count: LEARNED_FEATURE_COUNT,
            hidden: LEARNED_HIDDEN,
            value_scale: self.value_scale,
            policy_scale: self.policy_scale,
        }
    }

    fn value(&self, accumulator: i64) -> i32 {
        let score = (accumulator + self.value_bias) / i64::from(self.value_scale);
        score.clamp(-i64::from(EVALUATION_LIMIT), i64::from(EVALUATION_LIMIT)) as i32
    }

    fn policy(&self, side: Stone, keys: [LineKey; 4]) -> i32 {
        let score: i64 = keys
            .into_iter()
            .map(|key| self.policy_table[usize::from(relative_key(key, side).0)])
            .sum();
        (score / i64::from(self.policy_scale)).clamp(i64::from(i16::MIN), i64::from(i16::MAX))
            as i32
    }

    #[cfg(test)]
    fn deterministic_fixture() -> Self {
        let mut embeddings = vec![0_i16; LEARNED_FEATURE_COUNT * LEARNED_HIDDEN];
        for (index, value) in embeddings.iter_mut().enumerate() {
            *value = ((index as u64 * 17 + 11) % 31) as i16 - 15;
        }
        let value_head = std::array::from_fn(|index| index as i16 - 8);
        let policy_head = std::array::from_fn(|index| 8 - index as i16);
        let (value_table, policy_table) = compile_tables(&embeddings, &value_head, &policy_head);
        Self {
            embeddings: embeddings.into_boxed_slice(),
            fingerprint: [0; 32],
            value_head,
            policy_head,
            value_table,
            policy_table,
            value_bias: 37,
            value_scale: 64,
            policy_scale: 32,
        }
    }
}

// Distributivity is exact: each table entry is at most 16 * 32768^2;
// 900 entries fit in i64. No per-key division or clamp is permitted.
fn compile_tables(
    embeddings: &[i16],
    value_head: &[i16; LEARNED_HIDDEN],
    policy_head: &[i16; LEARNED_HIDDEN],
) -> (Box<[i64]>, Box<[i64]>) {
    let compile = |head: &[i16; LEARNED_HIDDEN]| {
        embeddings
            .as_chunks::<LEARNED_HIDDEN>()
            .0
            .iter()
            .map(|embedding| {
                embedding
                    .iter()
                    .zip(head)
                    .map(|(&a, &b)| i64::from(a) * i64::from(b))
                    .sum()
            })
            .collect::<Vec<i64>>()
            .into_boxed_slice()
    };
    (compile(value_head), compile(policy_head))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LearnedState {
    accumulators: [i64; 2],
}

#[derive(Clone, Debug)]
pub struct LearnedEvaluator {
    model: Arc<LearnedModel>,
}

impl LearnedEvaluator {
    #[must_use]
    pub fn new(model: Arc<LearnedModel>) -> Self {
        Self { model }
    }

    #[must_use]
    pub fn model(&self) -> &Arc<LearnedModel> {
        &self.model
    }

    /// Root/tooling convenience; online recursion keeps using SearchState's
    /// already initialized accumulator.
    #[must_use]
    pub fn evaluate_position(&self, position: &Position) -> i32 {
        let patterns = PatternState::new(position);
        let state = self.initialize(position, &patterns);
        self.evaluate(position, &patterns, &state)
    }

    /// Evaluate one legal root candidate without constructing a move list.
    #[must_use]
    pub fn policy_for(&self, position: &Position, at: Move) -> Option<i32> {
        let patterns = PatternState::new(position);
        let state = self.initialize(position, &patterns);
        self.policy_score(position, &patterns, &state, at)
    }

    fn apply_delta(&self, state: &mut LearnedState, delta: &PatternDelta, reverse: bool) {
        for (old, new) in delta.changes() {
            let (remove, add) = if reverse { (new, old) } else { (old, new) };
            for side in [Stone::Black, Stone::White] {
                let accumulator = &mut state.accumulators[stone_index(side)];
                *accumulator -= self.model.value_table[usize::from(relative_key(remove, side).0)];
                *accumulator += self.model.value_table[usize::from(relative_key(add, side).0)];
            }
        }
    }
}

impl Evaluator for LearnedEvaluator {
    fn model_fingerprint(&self) -> Option<[u8; 32]> {
        Some(self.model.fingerprint)
    }
    fn score_contract(&self) -> crate::ScoreContract {
        crate::ScoreContract::LinearV1
    }
    type State = LearnedState;
    type Undo = ();

    fn initialize(&self, _position: &Position, patterns: &PatternState) -> LearnedState {
        let mut state = LearnedState {
            accumulators: [0; 2],
        };
        for key in patterns.all_line_keys() {
            for side in [Stone::Black, Stone::White] {
                state.accumulators[stone_index(side)] +=
                    self.model.value_table[usize::from(relative_key(key, side).0)];
            }
        }
        state
    }

    fn make_move(&self, state: &mut LearnedState, delta: &PatternDelta) {
        self.apply_delta(state, delta, false);
    }

    fn unmake_move(&self, state: &mut LearnedState, delta: &PatternDelta, _undo: ()) {
        self.apply_delta(state, delta, true);
    }

    fn evaluate(&self, position: &Position, _patterns: &PatternState, state: &LearnedState) -> i32 {
        self.model
            .value(state.accumulators[stone_index(position.side_to_move())])
    }

    fn policy_score(
        &self,
        position: &Position,
        patterns: &PatternState,
        _state: &LearnedState,
        at: Move,
    ) -> Option<i32> {
        position.is_legal(at).then(|| {
            self.model
                .policy(position.side_to_move(), patterns.line_keys(at))
        })
    }
}

#[derive(Clone, Debug)]
pub enum RuntimeEvaluator {
    Pattern,
    Learned(LearnedEvaluator),
    Nonlinear(crate::NonlinearEvaluator),
}

impl RuntimeEvaluator {
    /// Stable diagnostic architecture name; distinct from the score contract.
    #[must_use]
    pub const fn architecture_name(&self) -> &'static str {
        match self {
            Self::Pattern => "pattern",
            Self::Learned(_) => "line-value-policy-v1",
            Self::Nonlinear(_) => "local-nonlinear-v2",
        }
    }

    #[must_use]
    pub const fn model_format_version(&self) -> Option<u16> {
        match self {
            Self::Pattern => None,
            Self::Learned(_) => Some(1),
            Self::Nonlinear(_) => Some(2),
        }
    }

    /// Load a explicitly selected model. Magic/version dispatch fails closed;
    /// neither floating references nor a damaged V2 file fall back to V1.
    pub fn read_from_path(path: impl AsRef<Path>) -> Result<Self, LearnedModelError> {
        let file = File::open(path).map_err(LearnedModelError::Io)?;
        let mut bytes = Vec::new();
        file.take(MAX_MODEL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(LearnedModelError::Io)?;
        Self::from_model_bytes(&bytes)
    }

    pub fn from_model_bytes(bytes: &[u8]) -> Result<Self, LearnedModelError> {
        match bytes.get(..8) {
            Some(b"RMLPV001") => Ok(Self::Learned(LearnedEvaluator::new(Arc::new(
                LearnedModel::read_from(&mut &*bytes)?,
            )))),
            Some(b"RMLPV002") => Ok(Self::Nonlinear(crate::NonlinearEvaluator::new(Arc::new(
                crate::NonlinearModel::read_from(&mut &*bytes)?,
            )))),
            _ => Err(LearnedModelError::Invalid(
                "unsupported runtime model magic",
            )),
        }
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeEvaluatorState {
    Pattern,
    Learned(LearnedState),
    Nonlinear(crate::NonlinearState),
}

pub enum RuntimeEvaluatorUndo {
    Pattern,
    Learned,
    Nonlinear,
}

impl Evaluator for RuntimeEvaluator {
    fn model_fingerprint(&self) -> Option<[u8; 32]> {
        match self {
            Self::Pattern => crate::PatternEvaluator.model_fingerprint(),
            Self::Learned(evaluator) => evaluator.model_fingerprint(),
            Self::Nonlinear(evaluator) => evaluator.model_fingerprint(),
        }
    }
    fn score_contract(&self) -> crate::ScoreContract {
        match self {
            Self::Pattern => crate::ScoreContract::Pattern,
            Self::Learned(evaluator) => evaluator.score_contract(),
            Self::Nonlinear(evaluator) => evaluator.score_contract(),
        }
    }
    type State = RuntimeEvaluatorState;
    type Undo = RuntimeEvaluatorUndo;

    fn initialize(&self, position: &Position, patterns: &PatternState) -> Self::State {
        match self {
            Self::Pattern => RuntimeEvaluatorState::Pattern,
            Self::Learned(evaluator) => {
                RuntimeEvaluatorState::Learned(evaluator.initialize(position, patterns))
            }
            Self::Nonlinear(evaluator) => {
                RuntimeEvaluatorState::Nonlinear(evaluator.initialize(position, patterns))
            }
        }
    }

    fn make_move(&self, state: &mut Self::State, delta: &PatternDelta) -> Self::Undo {
        match (self, state) {
            (Self::Pattern, RuntimeEvaluatorState::Pattern) => RuntimeEvaluatorUndo::Pattern,
            (Self::Learned(evaluator), RuntimeEvaluatorState::Learned(state)) => {
                evaluator.make_move(state, delta);
                RuntimeEvaluatorUndo::Learned
            }
            (Self::Nonlinear(evaluator), RuntimeEvaluatorState::Nonlinear(state)) => {
                evaluator.make_move(state, delta);
                RuntimeEvaluatorUndo::Nonlinear
            }
            _ => panic!("runtime evaluator state must match its immutable definition"),
        }
    }

    fn unmake_move(&self, state: &mut Self::State, delta: &PatternDelta, undo: Self::Undo) {
        match (self, state, undo) {
            (Self::Pattern, RuntimeEvaluatorState::Pattern, RuntimeEvaluatorUndo::Pattern) => {}
            (
                Self::Learned(evaluator),
                RuntimeEvaluatorState::Learned(state),
                RuntimeEvaluatorUndo::Learned,
            ) => evaluator.unmake_move(state, delta, ()),
            (
                Self::Nonlinear(evaluator),
                RuntimeEvaluatorState::Nonlinear(state),
                RuntimeEvaluatorUndo::Nonlinear,
            ) => evaluator.unmake_move(state, delta, ()),
            _ => panic!("runtime evaluator undo must match its immutable definition"),
        }
    }

    fn evaluate(&self, position: &Position, patterns: &PatternState, state: &Self::State) -> i32 {
        match (self, state) {
            (Self::Pattern, RuntimeEvaluatorState::Pattern) => {
                crate::PatternEvaluator.evaluate(position, patterns, &())
            }
            (Self::Learned(evaluator), RuntimeEvaluatorState::Learned(state)) => {
                evaluator.evaluate(position, patterns, state)
            }
            (Self::Nonlinear(evaluator), RuntimeEvaluatorState::Nonlinear(state)) => {
                evaluator.evaluate(position, patterns, state)
            }
            _ => panic!("runtime evaluator state must match its immutable definition"),
        }
    }

    fn policy_score(
        &self,
        position: &Position,
        patterns: &PatternState,
        state: &Self::State,
        at: Move,
    ) -> Option<i32> {
        match (self, state) {
            (Self::Pattern, RuntimeEvaluatorState::Pattern) => None,
            (Self::Learned(evaluator), RuntimeEvaluatorState::Learned(state)) => {
                evaluator.policy_score(position, patterns, state, at)
            }
            (Self::Nonlinear(evaluator), RuntimeEvaluatorState::Nonlinear(state)) => {
                evaluator.policy_score(position, patterns, state, at)
            }
            _ => panic!("runtime evaluator state must match its immutable definition"),
        }
    }
}

pub(crate) fn relative_key(key: LineKey, side: Stone) -> LineKey {
    if side == Stone::Black {
        return key;
    }
    let mut swapped = 0_u16;
    for field in 0..8 {
        let shift = field * 2;
        let value = (key.0 >> shift) & 3;
        let value = match value {
            1 => 2,
            2 => 1,
            other => other,
        };
        swapped |= value << shift;
    }
    LineKey(swapped)
}

#[derive(Debug)]
pub enum LearnedModelError {
    Io(io::Error),
    Invalid(&'static str),
}

impl fmt::Display for LearnedModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "learned model I/O error: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid learned model: {message}"),
        }
    }
}

impl std::error::Error for LearnedModelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn bytes(&mut self, count: usize) -> Result<&'a [u8], LearnedModelError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(LearnedModelError::Invalid("model offset overflow"))?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or(LearnedModelError::Invalid("truncated model"))?;
        self.offset = end;
        Ok(result)
    }
    fn u16(&mut self) -> Result<u16, LearnedModelError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn i16(&mut self) -> Result<i16, LearnedModelError> {
        Ok(i16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, LearnedModelError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, LearnedModelError> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, LearnedModelError> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search_state::SearchState;

    fn reference_value(model: &LearnedModel, patterns: &PatternState, side: Stone) -> i32 {
        let mut accumulator = [0_i64; LEARNED_HIDDEN];
        for key in patterns.all_line_keys() {
            let offset = usize::from(relative_key(key, side).0) * LEARNED_HIDDEN;
            for (dimension, sum) in accumulator.iter_mut().enumerate() {
                *sum += i64::from(model.embeddings[offset + dimension]);
            }
        }
        let dot = accumulator
            .iter()
            .zip(model.value_head)
            .fold(model.value_bias, |sum, (&feature, weight)| {
                sum + feature * i64::from(weight)
            });
        (dot / i64::from(model.value_scale))
            .clamp(-i64::from(EVALUATION_LIMIT), i64::from(EVALUATION_LIMIT)) as i32
    }

    #[test]
    fn folded_tables_match_unfolded_integer_reference_at_extremes() {
        for extreme in [false, true] {
            let mut model = LearnedModel::deterministic_fixture();
            if extreme {
                for (index, value) in model.embeddings.iter_mut().enumerate() {
                    *value = if index % 3 == 0 { i16::MIN } else { i16::MAX };
                }
                model.value_head = [i16::MIN; LEARNED_HIDDEN];
                model.policy_head = [i16::MAX; LEARNED_HIDDEN];
                model.value_bias = i64::MAX - MAX_VALUE_DOT;
                (model.value_table, model.policy_table) =
                    compile_tables(&model.embeddings, &model.value_head, &model.policy_head);
            }
            let evaluator = LearnedEvaluator::new(Arc::new(model));
            let mut position = Position::default();
            let mut undos = Vec::new();
            for index in (0..CELL_COUNT).map(|i| (i * 97) % CELL_COUNT).take(100) {
                let at = Move::from_index(index).unwrap();
                if !position.is_legal(at) {
                    break;
                }
                undos.push(position.make_move(at).unwrap());
                let patterns = PatternState::new(&position);
                let state = evaluator.initialize(&position, &patterns);
                for side in [Stone::Black, Stone::White] {
                    assert_eq!(
                        evaluator.model.value(state.accumulators[stone_index(side)]),
                        reference_value(&evaluator.model, &patterns, side)
                    );
                    for at in Move::all().filter(|&at| position.is_legal(at)) {
                        let mut dot = 0_i64;
                        for dimension in 0..LEARNED_HIDDEN {
                            let local: i64 = patterns
                                .line_keys(at)
                                .into_iter()
                                .map(|key| {
                                    i64::from(
                                        evaluator.model.embeddings[usize::from(
                                            relative_key(key, side).0,
                                        ) * LEARNED_HIDDEN
                                            + dimension],
                                    )
                                })
                                .sum();
                            dot += local * i64::from(evaluator.model.policy_head[dimension]);
                        }
                        let reference = (dot / i64::from(evaluator.model.policy_scale))
                            .clamp(i64::from(i16::MIN), i64::from(i16::MAX))
                            as i32;
                        assert_eq!(
                            evaluator.model.policy(side, patterns.line_keys(at)),
                            reference
                        );
                    }
                }
            }
            while let Some(undo) = undos.pop() {
                position.unmake_move(undo);
            }
            assert_eq!(position, Position::default());
        }
    }

    #[test]
    fn model_round_trip_and_parser_rejections() {
        let model = LearnedModel::deterministic_fixture();
        let mut bytes = Vec::new();
        model.write_to(&mut bytes).unwrap();
        let parsed = LearnedModel::read_from(&mut bytes.as_slice()).unwrap();
        assert_eq!(parsed.metadata(), model.metadata());
        assert_eq!(parsed.embeddings, model.embeddings);
        assert!(LearnedModel::read_from(&mut &bytes[..bytes.len() - 1]).is_err());
        bytes.push(0);
        assert!(LearnedModel::read_from(&mut bytes.as_slice()).is_err());
        let mut bad_magic = bytes;
        bad_magic[0] ^= 1;
        assert!(LearnedModel::read_from(&mut bad_magic.as_slice()).is_err());
        let mut bad_bias = {
            let mut bytes = Vec::new();
            LearnedModel::deterministic_fixture()
                .write_to(&mut bytes)
                .unwrap();
            bytes
        };
        bad_bias[24..32].copy_from_slice(&(i64::MAX - MAX_VALUE_DOT + 1).to_le_bytes());
        assert!(LearnedModel::read_from(&mut bad_bias.as_slice()).is_err());
        let oversized = vec![0_u8; MAX_MODEL_BYTES as usize + 1];
        assert!(LearnedModel::read_from(&mut oversized.as_slice()).is_err());
    }

    #[test]
    fn incremental_accumulator_matches_fresh_after_long_make_unmake() {
        let evaluator = LearnedEvaluator::new(Arc::new(LearnedModel::deterministic_fixture()));
        let original = Position::default();
        let mut state = SearchState::new(&original, &evaluator);
        let mut undos = Vec::new();
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..120 {
            let legal: Vec<_> = Move::all()
                .filter(|&at| state.position().is_legal(at))
                .collect();
            if legal.is_empty() {
                break;
            }
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = legal[(seed % legal.len() as u64) as usize];
            undos.push(state.make_move(at, &evaluator).unwrap());
            state.assert_consistent(&evaluator);
        }
        while let Some(undo) = undos.pop() {
            state.unmake_move(undo, &evaluator);
            state.assert_consistent(&evaluator);
        }
        assert_eq!(state.position(), &original);
    }

    #[test]
    fn quantized_value_and_policy_stay_in_documented_ranges() {
        let evaluator = LearnedEvaluator::new(Arc::new(LearnedModel::deterministic_fixture()));
        let position = Position::default();
        let patterns = PatternState::new(&position);
        let state = evaluator.initialize(&position, &patterns);
        assert!(evaluator.evaluate(&position, &patterns, &state).abs() <= EVALUATION_LIMIT);
        let policy = evaluator
            .policy_score(&position, &patterns, &state, Move::CENTER)
            .unwrap();
        assert!((i32::from(i16::MIN)..=i32::from(i16::MAX)).contains(&policy));
    }

    #[test]
    fn value_selects_the_side_to_move_relative_accumulator() {
        let mut model = LearnedModel::deterministic_fixture();
        model.value_head = [0; LEARNED_HIDDEN];
        model.value_head[0] = 1;
        model.value_bias = 0;
        model.value_scale = 1;
        let evaluator = LearnedEvaluator::new(Arc::new(model));
        let mut state = LearnedState {
            accumulators: [0; 2],
        };
        state.accumulators[stone_index(Stone::Black)] = 123;
        state.accumulators[stone_index(Stone::White)] = -123;
        let black_to_move = Position::default();
        let black_patterns = PatternState::new(&black_to_move);
        assert_eq!(
            evaluator.evaluate(&black_to_move, &black_patterns, &state),
            123
        );
        let mut white_to_move = Position::default();
        white_to_move.make_move(Move::CENTER).unwrap();
        let white_patterns = PatternState::new(&white_to_move);
        assert_eq!(
            evaluator.evaluate(&white_to_move, &white_patterns, &state),
            -123
        );
    }

    #[test]
    fn learned_accumulator_is_worker_local_in_t8_smoke() {
        use crate::{AlphaBetaEngine, EngineConfig, SearchEngine, SearchLimits, SearchTermination};

        let evaluator = LearnedEvaluator::new(Arc::new(LearnedModel::deterministic_fixture()));
        let config = EngineConfig::new(1)
            .with_threads(8)
            .with_vcf_limits(0, 0)
            .with_vct_limits(0, 0)
            .with_vct_table_memory(0);
        let result = AlphaBetaEngine::with_config(evaluator, config)
            .search(&Position::default(), SearchLimits::new(2));
        assert_eq!(result.termination, SearchTermination::Completed);
        assert_eq!(result.statistics.worker_count, 8);
        assert!(
            result
                .best_move
                .is_some_and(|at| Position::default().is_legal(at))
        );
    }
}
