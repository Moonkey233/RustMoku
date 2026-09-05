use crate::{
    pattern::{ThreatProfile, stone_index},
    pattern_state::{PatternState, PatternUndo},
};
use rustmoku_core::{Move, Position, Stone};

const DIRECTIONS: [(isize, isize); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];
const FIVE: i32 = 1_000_000;
const OPEN_FOUR: i32 = 100_000;
const CLOSED_FOUR: i32 = 10_000;
const OPEN_THREE: i32 = 2_000;
const CLOSED_THREE: i32 = 200;
const OPEN_TWO: i32 = 50;
const CLOSED_TWO: i32 = 5;
const EVALUATION_LIMIT: i32 = 10_000_000;

/// Static position scoring from the side-to-move perspective.
pub trait Evaluator {
    /// Owned per-search state. Evaluator configuration remains immutable.
    type State;
    /// Consumed in strict LIFO order on the corresponding logical state.
    type Undo;

    fn initialize(&self, position: &Position) -> Self::State;
    /// Called only after Core has accepted the move; must complete infallibly.
    fn make_move(&self, state: &mut Self::State, at: Move, stone: Stone) -> Self::Undo;
    fn unmake_move(&self, state: &mut Self::State, undo: Self::Undo);
    /// Static score, positive for side to move, strictly outside the mate range.
    fn evaluate(&self, position: &Position, state: &Self::State) -> i32;

    /// Reuses tactical state for ordering when available. Presence must remain
    /// constant throughout the lifecycle. Other evaluators get an independent
    /// engine-owned pattern cache, while ClassicalEvaluator keeps State=().
    fn cached_patterns(_state: &Self::State) -> Option<&PatternState> {
        None
    }
}

/// A deliberately simple contiguous-run evaluator used as the V0.1 baseline.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClassicalEvaluator;

impl Evaluator for ClassicalEvaluator {
    type State = ();
    type Undo = ();

    fn initialize(&self, _position: &Position) {}
    fn make_move(&self, _state: &mut (), _at: Move, _stone: Stone) {}
    fn unmake_move(&self, _state: &mut (), _undo: ()) {}

    fn evaluate(&self, position: &Position, _state: &()) -> i32 {
        let side = position.side_to_move();
        let score = score_stone(position, side) - score_stone(position, side.opponent());
        score.clamp(-EVALUATION_LIMIT, EVALUATION_LIMIT)
    }
}

/// Incremental Freestyle threats with initial, untuned feature weights.
#[derive(Clone, Copy, Debug, Default)]
pub struct PatternEvaluator;

// Quiet, Three, OpenThree, DoubleThree, Four, FourThree, DoubleFour,
// OpenFour, WinningMove. Semantics live in pattern.rs, not in these weights.
const PATTERN_WEIGHTS: [i32; ThreatProfile::COUNT] = [
    0, 20, 200, 2_000, 10_000, 50_000, 100_000, 120_000, 1_000_000,
];

impl Evaluator for PatternEvaluator {
    type State = PatternState;
    type Undo = PatternUndo;

    fn cached_patterns(state: &PatternState) -> Option<&PatternState> {
        Some(state)
    }

    fn initialize(&self, position: &Position) -> PatternState {
        PatternState::new(position)
    }
    fn make_move(&self, state: &mut PatternState, at: Move, stone: Stone) -> PatternUndo {
        state.make_move(at, stone)
    }
    fn unmake_move(&self, state: &mut PatternState, undo: PatternUndo) {
        state.unmake_move(undo);
    }
    fn evaluate(&self, position: &Position, state: &PatternState) -> i32 {
        let side = stone_index(position.side_to_move());
        let counts = state.counts();
        let mut score = 0;
        for (feature, &weight) in PATTERN_WEIGHTS.iter().enumerate() {
            score +=
                (i32::from(counts[side][feature]) - i32::from(counts[1 - side][feature])) * weight;
        }
        score.clamp(-EVALUATION_LIMIT, EVALUATION_LIMIT)
    }
}

fn score_stone(position: &Position, stone: Stone) -> i32 {
    let mut score: i32 = 0;

    for at in Move::all().filter(|&at| position.cell(at) == Some(stone)) {
        for (row_step, column_step) in DIRECTIONS {
            if offset_move(at, -row_step, -column_step)
                .is_some_and(|previous| position.cell(previous) == Some(stone))
            {
                continue;
            }

            let mut length = 1;
            let mut end = at;
            while let Some(next) = offset_move(end, row_step, column_step) {
                if position.cell(next) != Some(stone) {
                    break;
                }
                length += 1;
                end = next;
            }

            let open_before = offset_move(at, -row_step, -column_step)
                .is_some_and(|before| position.cell(before).is_none());
            let open_after = offset_move(end, row_step, column_step)
                .is_some_and(|after| position.cell(after).is_none());
            score = score.saturating_add(pattern_score(
                length,
                usize::from(open_before) + usize::from(open_after),
            ));
        }
    }

    score
}

const fn pattern_score(length: usize, open_ends: usize) -> i32 {
    match (length, open_ends) {
        (5.., _) => FIVE,
        (4, 2) => OPEN_FOUR,
        (4, 1) => CLOSED_FOUR,
        (3, 2) => OPEN_THREE,
        (3, 1) => CLOSED_THREE,
        (2, 2) => OPEN_TWO,
        (2, 1) => CLOSED_TWO,
        _ => 0,
    }
}

fn offset_move(at: Move, row_step: isize, column_step: isize) -> Option<Move> {
    let row = at.row().checked_add_signed(row_step)?;
    let column = at.column().checked_add_signed(column_step)?;
    Move::from_row_col(row, column).ok()
}
