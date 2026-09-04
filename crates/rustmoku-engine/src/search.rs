use rustmoku_core::{Move, Position};

use crate::{ClassicalEvaluator, Evaluator, generate_candidates, move_ordering::order_moves};

const MATE_SCORE: i32 = 100_000_000;
const SEARCH_INFINITY: i32 = 200_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_depth: u8,
}

impl SearchLimits {
    pub const DEFAULT_DEPTH: u8 = 4;

    #[must_use]
    pub const fn new(max_depth: u8) -> Self {
        Self { max_depth }
    }
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self::new(Self::DEFAULT_DEPTH)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub best_move: Option<Move>,
    pub score: i32,
    pub requested_depth: u8,
    pub reached_depth: u8,
    pub nodes: u64,
}

pub trait SearchEngine {
    fn search(&self, position: &Position, limits: SearchLimits) -> SearchResult;
}

#[derive(Debug)]
pub struct AlphaBetaEngine<E = ClassicalEvaluator> {
    evaluator: E,
}

impl<E> AlphaBetaEngine<E> {
    #[must_use]
    pub const fn new(evaluator: E) -> Self {
        Self { evaluator }
    }
}

impl Default for AlphaBetaEngine<ClassicalEvaluator> {
    fn default() -> Self {
        Self::new(ClassicalEvaluator)
    }
}

impl<E: Evaluator> SearchEngine for AlphaBetaEngine<E> {
    fn search(&self, position: &Position, limits: SearchLimits) -> SearchResult {
        // This is the one intentional Position clone: callers retain immutable
        // ownership while the whole recursive search reuses this working copy.
        let mut working = position.clone();
        let mut statistics = SearchStatistics {
            nodes: 1,
            max_ply: 0,
        };

        if let Some(score) = terminal_score(&working, 0) {
            return result(None, score, limits, statistics);
        }
        if limits.max_depth == 0 {
            return result(None, self.evaluator.evaluate(&working), limits, statistics);
        }

        let mut moves = generate_candidates(&working);
        order_moves(&working, &mut moves);
        let mut best_move = None;
        let mut best_score = -SEARCH_INFINITY;
        let mut alpha = -SEARCH_INFINITY;

        for at in moves.iter() {
            let Ok(undo) = working.make_move(at) else {
                continue;
            };
            let score = -self.negamax(
                &mut working,
                limits.max_depth - 1,
                -SEARCH_INFINITY,
                -alpha,
                1,
                &mut statistics,
            );
            working.unmake_move(undo);

            if score > best_score {
                best_score = score;
                best_move = Some(at);
            }
            alpha = alpha.max(score);
        }

        if best_move.is_none() {
            best_score = 0;
        }
        result(best_move, best_score, limits, statistics)
    }
}

impl<E: Evaluator> AlphaBetaEngine<E> {
    fn negamax(
        &self,
        position: &mut Position,
        depth: u8,
        mut alpha: i32,
        beta: i32,
        ply: u8,
        statistics: &mut SearchStatistics,
    ) -> i32 {
        statistics.nodes += 1;
        statistics.max_ply = statistics.max_ply.max(ply);

        if let Some(score) = terminal_score(position, ply) {
            return score;
        }
        if depth == 0 {
            return self.evaluator.evaluate(position);
        }

        let mut moves = generate_candidates(position);
        if moves.is_empty() {
            return 0;
        }
        order_moves(position, &mut moves);
        let mut best_score = -SEARCH_INFINITY;

        for at in moves.iter() {
            let Ok(undo) = position.make_move(at) else {
                continue;
            };
            let score = -self.negamax(position, depth - 1, -beta, -alpha, ply + 1, statistics);
            position.unmake_move(undo);

            best_score = best_score.max(score);
            alpha = alpha.max(score);
            if alpha >= beta {
                break;
            }
        }

        best_score
    }
}

#[derive(Debug)]
struct SearchStatistics {
    nodes: u64,
    max_ply: u8,
}

fn terminal_score(position: &Position, ply: u8) -> Option<i32> {
    if let Some(winner) = position.winner() {
        let distance = i32::from(ply);
        return Some(if winner == position.side_to_move() {
            MATE_SCORE - distance
        } else {
            -MATE_SCORE + distance
        });
    }
    position.is_full().then_some(0)
}

const fn result(
    best_move: Option<Move>,
    score: i32,
    limits: SearchLimits,
    statistics: SearchStatistics,
) -> SearchResult {
    SearchResult {
        best_move,
        score,
        requested_depth: limits.max_depth,
        reached_depth: statistics.max_ply,
        nodes: statistics.nodes,
    }
}
