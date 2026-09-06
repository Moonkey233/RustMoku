//! Exact Freestyle continuous-four proofs, independent of static evaluation.
use rustmoku_core::{Move, Stone};

use crate::{
    board_state::BoardState,
    principal_variation::PvTable,
    proof_table::{CachedProof, ProofTable, solver_key},
    search_control::{ProofResources, SearchBudget, Stopped},
    tactical::{ImmediateTactic, forcing_moves, immediate_tactic},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VcfStatus {
    ProvenWin {
        plies: u8,
    },
    /// No proof within the continuous-four definition and requested depth.
    NotProven,
    BudgetExceeded,
    Interrupted,
}

#[derive(Debug)]
pub(crate) struct VcfResult {
    pub(crate) status: VcfStatus,
    pub(crate) principal_variation: Vec<Move>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VcfStatistics {
    pub(crate) nodes: u64,
    pub(crate) cache_hits: u64,
    pub(crate) probes: u64,
    pub(crate) proven: u64,
    pub(crate) budget_exhausted: u64,
}

pub(crate) struct VcfSolver {
    table: ProofTable,
    remaining_nodes: u64,
    statistics: VcfStatistics,
}

impl VcfSolver {
    pub(crate) fn new() -> Self {
        Self {
            table: ProofTable::new(),
            remaining_nodes: 0,
            statistics: VcfStatistics::default(),
        }
    }

    pub(crate) fn begin_search(&mut self, max_nodes: u64) {
        self.table.begin_search();
        self.remaining_nodes = max_nodes;
        self.statistics = VcfStatistics::default();
    }

    pub(crate) fn statistics(&self) -> VcfStatistics {
        self.statistics
    }

    #[cfg(test)]
    pub(crate) fn solve(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        max_plies: u8,
    ) -> VcfResult {
        self.solve_controlled(board, attacker, max_plies, &mut SearchBudget::default())
    }

    /// Every return path restores all board fields; the evaluator is absent.
    pub(crate) fn solve_controlled(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        max_plies: u8,
        budget: &mut SearchBudget,
    ) -> VcfResult {
        self.statistics.probes += 1;
        let mut pv = PvTable::new();
        let available = (rustmoku_core::CELL_COUNT - board.position().move_count()) as u8;
        let max_plies = max_plies.min(available);
        let mut status = VcfStatus::NotProven;
        // Increasing terminal distance, then ascending attacks: a longer line
        // can never hide a shorter proof or win an equal-distance root tie.
        let terminal = board.position().winner().is_some() || board.position().is_full();
        let first = if terminal {
            0
        } else if board.position().side_to_move() == attacker {
            1
        } else {
            2
        };
        for depth in (first..=max_plies).step_by(2) {
            status = self.visit(
                board,
                attacker,
                depth,
                0,
                &mut ProofResources {
                    pv: &mut pv,
                    budget,
                },
            );
            if status != VcfStatus::NotProven {
                break;
            }
        }
        let principal_variation = match status {
            VcfStatus::ProvenWin { plies } => {
                self.statistics.proven += 1;
                debug_assert_eq!(pv.root_line().len(), usize::from(plies));
                pv.root_line().to_vec()
            }
            VcfStatus::BudgetExceeded => {
                self.statistics.budget_exhausted += 1;
                Vec::new()
            }
            VcfStatus::NotProven | VcfStatus::Interrupted => Vec::new(),
        };
        VcfResult {
            status,
            principal_variation,
        }
    }

    fn visit(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        depth: u8,
        ply: u8,
        resources: &mut ProofResources<'_>,
    ) -> VcfStatus {
        resources.pv.clear(ply);
        // This dedicated remaining budget controls work; counters only observe it.
        // A visit, including a cache hit or a depth-zero node, costs one node.
        if self.remaining_nodes == 0 {
            return VcfStatus::BudgetExceeded;
        }
        if resources.budget.charge().is_err() {
            return VcfStatus::Interrupted;
        }
        self.remaining_nodes -= 1;
        self.statistics.nodes += 1;
        let key = solver_key(board.key(), attacker);
        if let Some(entry) = self.table.probe(key) {
            match entry.proof {
                CachedProof::NotProven if entry.depth >= depth => {
                    self.statistics.cache_hits += 1;
                    return VcfStatus::NotProven;
                }
                CachedProof::ProvenWin { plies } if plies <= depth => {
                    // A collision may have evicted descendants. Never return a
                    // partial certificate: missing links fall back to search.
                    match self.replay(board, attacker, depth, ply, resources) {
                        Ok(Some(distance)) if distance == plies => {
                            self.statistics.cache_hits += 1;
                            return VcfStatus::ProvenWin { plies };
                        }
                        Err(Stopped) => return VcfStatus::Interrupted,
                        Ok(_) => {}
                    }
                    resources.pv.clear(ply);
                }
                _ => {}
            }
        }
        let result = self.expand(board, attacker, depth, ply, resources);
        let cached = match result {
            VcfStatus::ProvenWin { plies } => CachedProof::ProvenWin { plies },
            VcfStatus::NotProven => CachedProof::NotProven,
            VcfStatus::BudgetExceeded | VcfStatus::Interrupted => return result,
        };
        self.table
            .store(key, depth, cached, resources.pv.line(ply).first().copied());
        result
    }

    fn expand(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        depth: u8,
        ply: u8,
        resources: &mut ProofResources<'_>,
    ) -> VcfStatus {
        if let Some(fact) = resolve_fact(board, attacker, depth, ply, resources.pv) {
            return fact;
        }
        if depth == 0 {
            return VcfStatus::NotProven;
        }
        if board.position().side_to_move() != attacker {
            let Some(at) = immediate_tactic(board.patterns(), attacker.opponent()).forced_block()
            else {
                return VcfStatus::NotProven;
            };
            return self.continue_at(board, attacker, at, depth, ply, resources);
        }
        for at in forcing_moves(board.patterns(), attacker).iter() {
            // The defender node rechecks actual resulting winning cells, with
            // defender counter-wins first. Pre-move profiles alone prove nothing.
            let result = self.continue_at(board, attacker, at, depth, ply, resources);
            if result != VcfStatus::NotProven {
                return result;
            }
        }
        VcfStatus::NotProven
    }

    fn continue_at(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        at: Move,
        depth: u8,
        ply: u8,
        resources: &mut ProofResources<'_>,
    ) -> VcfStatus {
        let undo = board.make_move(at).expect("tactical bitset move is legal");
        let child = self.visit(board, attacker, depth - 1, ply + 1, resources);
        board.unmake_move(undo);
        match child {
            VcfStatus::ProvenWin { plies } => {
                resources.pv.update(ply, at);
                VcfStatus::ProvenWin { plies: plies + 1 }
            }
            other => other,
        }
    }

    /// Bounded certificate reconstruction, not expansion: at most `depth`
    /// transitions, no branching. Each visit spends global work, but no local expansion budget.
    /// Every link is validated against actual board facts and restored on return.
    fn replay(
        &self,
        board: &mut BoardState,
        attacker: Stone,
        depth: u8,
        ply: u8,
        resources: &mut ProofResources<'_>,
    ) -> Result<Option<u8>, Stopped> {
        resources.budget.charge()?;
        resources.pv.clear(ply);
        if let Some(fact) = resolve_fact(board, attacker, depth, ply, resources.pv) {
            return Ok(match fact {
                VcfStatus::ProvenWin { plies } => Some(plies),
                _ => None,
            });
        }
        if depth == 0 {
            return Ok(None);
        }
        let Some(entry) = self.table.probe(solver_key(board.key(), attacker)) else {
            return Ok(None);
        };
        let CachedProof::ProvenWin { plies } = entry.proof else {
            return Ok(None);
        };
        if plies > depth {
            return Ok(None);
        }
        let Some(at) = entry.best_move.filter(|&at| board.position().is_legal(at)) else {
            return Ok(None);
        };
        if board.position().side_to_move() == attacker {
            if !forcing_moves(board.patterns(), attacker).test(at) {
                return Ok(None);
            }
        } else if immediate_tactic(board.patterns(), attacker.opponent()).forced_block() != Some(at)
        {
            return Ok(None);
        }
        let undo = board.make_move(at).expect("validated cached move");
        let child = self.replay(board, attacker, depth - 1, ply + 1, resources);
        board.unmake_move(undo);
        if child?.is_some_and(|distance| distance + 1 == plies) {
            resources.pv.update(ply, at);
            Ok(Some(plies))
        } else {
            Ok(None)
        }
    }
}

fn resolve_fact(
    board: &BoardState,
    attacker: Stone,
    depth: u8,
    ply: u8,
    pv: &mut PvTable,
) -> Option<VcfStatus> {
    if let Some(winner) = board.position().winner() {
        return Some(if winner == attacker {
            VcfStatus::ProvenWin { plies: 0 }
        } else {
            VcfStatus::NotProven
        });
    }
    if board.position().is_full() {
        return Some(VcfStatus::NotProven);
    }
    let side = board.position().side_to_move();
    let tactic = immediate_tactic(board.patterns(), side);
    let distance = if side == attacker {
        match tactic {
            ImmediateTactic::Win(_) => 1,
            ImmediateTactic::Loss { .. } => return Some(VcfStatus::NotProven),
            _ => return None,
        }
    } else {
        match tactic {
            // Defender wins before the attacker can play a winning cell.
            ImmediateTactic::Win(_) | ImmediateTactic::None => return Some(VcfStatus::NotProven),
            ImmediateTactic::Loss { .. } => 2,
            ImmediateTactic::ForcedBlock(_) => return None,
        }
    };
    if distance > depth {
        return Some(VcfStatus::NotProven);
    }
    tactic.resolve(ply, pv, &mut 0);
    Some(VcfStatus::ProvenWin { plies: distance })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AlphaBetaEngine, EngineConfig, Evaluator, PatternEvaluator, SearchEngine, SearchLimits,
        search_state::SearchState,
    };
    use rustmoku_core::Position;

    // 111 forces 112; 96 then creates two independent vertical winning cells.
    const CHAIN: &[usize] = &[108, 107, 109, 0, 110, 2, 66, 4, 81, 6];

    #[test]
    fn outer_interruption_and_cached_replay_restore_board_without_disproofs() {
        let position = fixture(CHAIN);
        let mut board = BoardState::new(&position);
        let mut solver = VcfSolver::new();
        for cap in [1, 5, 18] {
            solver.begin_search(100_000);
            let mut budget = SearchBudget::new(
                SearchLimits::new(1).with_max_nodes(cap),
                crate::CancellationToken::new(),
            );
            let result = solver.solve_controlled(&mut board, Stone::Black, 5, &mut budget);
            assert_eq!(result.status, VcfStatus::Interrupted);
            assert_eq!(solver.statistics().budget_exhausted, 0);
            assert_eq!(board, BoardState::new(&position));
            assert!(
                solver
                    .table
                    .probe(solver_key(board.key(), Stone::Black))
                    .is_none_or(|entry| entry.depth < 5 || entry.proof != CachedProof::NotProven)
            );
            // Same generation, so a false cached disproof would remain visible.
            verify(
                &position,
                &solver.solve(&mut board, Stone::Black, 5),
                5,
                111,
            );
        }
        let mut budget = SearchBudget::new(
            SearchLimits::new(1).with_max_nodes(2),
            crate::CancellationToken::new(),
        );
        let status = solver.visit(
            &mut board,
            Stone::Black,
            5,
            0,
            &mut ProofResources {
                pv: &mut PvTable::new(),
                budget: &mut budget,
            },
        );
        assert_eq!(status, VcfStatus::Interrupted);
        assert_eq!(board, BoardState::new(&position));
        verify(
            &position,
            &solver.solve(&mut board, Stone::Black, 5),
            5,
            111,
        );
    }

    fn at(index: usize) -> Move {
        Move::from_index(index).unwrap()
    }
    fn fixture(moves: &[usize]) -> Position {
        let mut position = Position::default();
        for &index in moves {
            position.make_move(at(index)).unwrap();
        }
        position
    }
    fn solve(position: &Position, depth: u8, nodes: u64) -> VcfResult {
        let mut solver = VcfSolver::new();
        solver.begin_search(nodes);
        solver.solve(
            &mut BoardState::new(position),
            position.side_to_move(),
            depth,
        )
    }
    fn verify(position: &Position, result: &VcfResult, plies: u8, best: usize) {
        assert_eq!(result.status, VcfStatus::ProvenWin { plies });
        assert_eq!(result.principal_variation.len(), usize::from(plies));
        assert_eq!(result.principal_variation.first(), Some(&at(best)));
        let mut replay = position.clone();
        for &at in &result.principal_variation {
            replay.make_move(at).unwrap();
        }
        assert_eq!(replay.winner(), Some(position.side_to_move()));
    }

    #[test]
    fn immediate_win_and_terminal_facts_obey_the_proof_depth() {
        let mut position = fixture(&[109, 0, 110, 2, 112, 4, 113, 6]);
        assert_eq!(solve(&position, 0, 100).status, VcfStatus::NotProven);
        verify(&position, &solve(&position, 1, 100), 1, 111);
        position.make_move(at(111)).unwrap();
        let mut solver = VcfSolver::new();
        solver.begin_search(100);
        assert_eq!(
            solver
                .solve(&mut BoardState::new(&position), Stone::Black, 0)
                .status,
            VcfStatus::ProvenWin { plies: 0 }
        );
        assert_eq!(
            solver
                .solve(&mut BoardState::new(&position), Stone::White, 0)
                .status,
            VcfStatus::NotProven
        );
    }

    #[test]
    fn multi_step_continuous_four_proof_is_shortest_and_cache_pv_is_complete() {
        let position = fixture(CHAIN);
        assert_eq!(solve(&position, 4, 10_000).status, VcfStatus::NotProven);
        let mut board = BoardState::new(&position);
        let mut solver = VcfSolver::new();
        solver.begin_search(10_000);
        let result = solver.solve(&mut board, Stone::Black, 11);
        verify(&position, &result, 5, 111);
        assert_eq!(
            &result.principal_variation[..3],
            &[at(111), at(112), at(96)]
        );
        let before = solver.statistics();
        let mut cached_pv = PvTable::new();
        assert_eq!(
            solver.visit(
                &mut board,
                Stone::Black,
                5,
                0,
                &mut ProofResources {
                    pv: &mut cached_pv,
                    budget: &mut SearchBudget::default()
                }
            ),
            VcfStatus::ProvenWin { plies: 5 }
        );
        assert_eq!(cached_pv.root_line(), result.principal_variation);
        assert_eq!(solver.statistics().nodes - before.nodes, 1);
        let cached = solver.solve(&mut board, Stone::Black, 11);
        verify(&position, &cached, 5, 111);
        assert_eq!(cached.principal_variation, result.principal_variation);
        assert!(solver.statistics().cache_hits > before.cache_hits);
        // Missing descendants must not truncate the winning PV. Keep only root.
        let key = solver_key(board.key(), Stone::Black);
        let entry = solver.table.probe(key).unwrap();
        solver.table.begin_search();
        solver
            .table
            .store(key, entry.depth, entry.proof, entry.best_move);
        verify(
            &position,
            &solver.solve(&mut board, Stone::Black, 11),
            5,
            111,
        );
    }

    #[test]
    fn parity_skips_impossible_depths_and_proven_cache_accepts_larger_caps() {
        let position = fixture(CHAIN);
        let mut board = BoardState::new(&position);
        let mut solver = VcfSolver::new();
        solver.begin_search(10_000);
        let proof = solver.solve(&mut board, Stone::Black, 5);
        verify(&position, &proof, 5, 111);
        let before = solver.statistics();
        let mut pv = PvTable::new();
        assert_eq!(
            solver.visit(
                &mut board,
                Stone::Black,
                9,
                0,
                &mut ProofResources {
                    pv: &mut pv,
                    budget: &mut SearchBudget::default()
                }
            ),
            VcfStatus::ProvenWin { plies: 5 }
        );
        assert_eq!(solver.statistics().nodes - before.nodes, 1);
        assert_eq!(solver.statistics().cache_hits - before.cache_hits, 1);
        assert_eq!(pv.root_line(), proof.principal_variation);
        // A depth-zero nonterminal solve and an odd defender cap do no work.
        solver.begin_search(10_000);
        assert_eq!(
            solver.solve(&mut board, Stone::Black, 0).status,
            VcfStatus::NotProven
        );
        assert_eq!(solver.statistics().nodes, 0);
        let undo = board.make_move(at(111)).unwrap();
        assert_eq!(
            solver.solve(&mut board, Stone::Black, 1).status,
            VcfStatus::NotProven
        );
        assert_eq!(solver.statistics().nodes, 0);
        let result = solver.solve(&mut board, Stone::Black, 4);
        assert_eq!(result.status, VcfStatus::ProvenWin { plies: 4 });
        let defender_nodes = solver.statistics().nodes;
        solver.begin_search(10_000);
        let mut manual = PvTable::new();
        assert_eq!(
            solver.visit(
                &mut board,
                Stone::Black,
                2,
                0,
                &mut ProofResources {
                    pv: &mut manual,
                    budget: &mut SearchBudget::default()
                }
            ),
            VcfStatus::NotProven
        );
        assert_eq!(
            solver.visit(
                &mut board,
                Stone::Black,
                4,
                0,
                &mut ProofResources {
                    pv: &mut manual,
                    budget: &mut SearchBudget::default()
                }
            ),
            VcfStatus::ProvenWin { plies: 4 }
        );
        assert_eq!(solver.statistics().nodes, defender_nodes);
        board.unmake_move(undo);
        assert_eq!(board, BoardState::new(&position));
    }

    #[test]
    fn open_four_and_double_four_use_distinct_winning_cells_and_canonical_ties() {
        for (position, best) in [
            (fixture(&[110, 0, 111, 2, 112, 4]), 109),
            (
                fixture(&[109, 0, 110, 2, 113, 4, 67, 6, 82, 8, 127, 10]),
                112,
            ),
        ] {
            verify(&position, &solve(&position, 11, 10_000), 3, best);
        }
    }

    #[test]
    fn defender_immediate_counter_win_refutes_even_an_open_four() {
        let position = fixture(&[110, 1, 111, 2, 112, 3, 0, 4]);
        let mut board = BoardState::new(&position);
        let undo = board.make_move(at(109)).unwrap();
        assert_eq!(
            board.patterns().winning_moves(Stone::Black).iter().count(),
            2
        );
        assert!(!board.patterns().winning_moves(Stone::White).is_empty());
        let mut solver = VcfSolver::new();
        solver.begin_search(10_000);
        assert_eq!(
            solver.solve(&mut board, Stone::Black, 11).status,
            VcfStatus::NotProven
        );
        board.unmake_move(undo);
        assert_eq!(solve(&position, 11, 10_000).status, VcfStatus::NotProven);
    }

    #[test]
    fn attack_without_a_resulting_four_is_rejected() {
        let position = fixture(CHAIN);
        let mut board = BoardState::new(&position);
        let undo = board.make_move(at(96)).unwrap();
        assert!(board.patterns().winning_moves(Stone::Black).is_empty());
        let mut solver = VcfSolver::new();
        solver.begin_search(1_000);
        assert_eq!(
            solver.solve(&mut board, Stone::Black, 11).status,
            VcfStatus::NotProven
        );
        board.unmake_move(undo);
        assert_eq!(board, BoardState::new(&position));
    }

    #[test]
    fn shorter_proof_precedes_a_smaller_index_longer_attack() {
        let mut position = fixture(CHAIN);
        for index in [171, 20, 172, 22, 173, 24] {
            position.make_move(at(index)).unwrap();
        }
        assert!(forcing_moves(BoardState::new(&position).patterns(), Stone::Black).test(at(111)));
        verify(&position, &solve(&position, 11, 10_000), 3, 170);
    }

    #[test]
    fn budget_exhaustion_never_becomes_a_cached_no_proof() {
        let position = fixture(CHAIN);
        let mut board = BoardState::new(&position);
        let key = solver_key(board.key(), Stone::Black);
        let mut solver = VcfSolver::new();
        solver.begin_search(1);
        assert_eq!(
            solver.solve(&mut board, Stone::Black, 11).status,
            VcfStatus::BudgetExceeded
        );
        assert_eq!(solver.statistics().nodes, 1);
        assert_eq!(solver.statistics().budget_exhausted, 1);
        assert!(solver.table.probe(key).is_none());
        // Test-only replenishment keeps this generation to detect a poisoned
        // negative entry. Production budgets only reset at begin_search.
        solver.remaining_nodes = 10_000;
        verify(
            &position,
            &solver.solve(&mut board, Stone::Black, 11),
            5,
            111,
        );
    }

    #[test]
    fn public_search_is_deterministic_across_warm_cache_and_other_search_history() {
        let position = fixture(CHAIN);
        for nodes in [1, 20, 2_000] {
            let config = EngineConfig::new(1).with_vcf_limits(11, nodes);
            let mut engine = AlphaBetaEngine::with_config(PatternEvaluator, config);
            let cold = engine.search(&position, SearchLimits::new(2));
            engine.search(&fixture(&[110, 0, 111, 2, 112, 4]), SearchLimits::new(2));
            let warm = engine.search(&position, SearchLimits::new(2));
            assert_eq!(
                (cold.best_move, cold.score, cold.proof, cold.completed_depth),
                (warm.best_move, warm.score, warm.proof, warm.completed_depth)
            );
            assert_eq!(
                (
                    cold.statistics.vcf_nodes,
                    cold.statistics.vcf_cache_hits,
                    cold.statistics.vcf_budget_exhausted
                ),
                (
                    warm.statistics.vcf_nodes,
                    warm.statistics.vcf_cache_hits,
                    warm.statistics.vcf_budget_exhausted
                )
            );
            if nodes == 2_000 {
                assert_eq!(cold.proof.unwrap().distance, crate::ProofDistance::Exact(5));
                assert_eq!(cold.completed_depth, 0);
                assert_eq!(cold.seldepth, 5);
                assert_eq!(cold.score, crate::score::MATE_SCORE - 5);
                assert_eq!(cold.principal_variation, warm.principal_variation);
            }
        }
    }

    #[test]
    fn all_solver_exits_restore_board_and_never_access_evaluator_state() {
        struct InaccessibleEvaluator;
        impl Evaluator for InaccessibleEvaluator {
            type State = ();
            type Undo = ();
            fn initialize(&self, _: &Position) {}
            fn make_move(&self, _: &mut (), _: Move, _: Stone) {
                panic!("proof accessed evaluator");
            }
            fn unmake_move(&self, _: &mut (), _: ()) {
                panic!("proof accessed evaluator");
            }
            fn evaluate(&self, _: &Position, _: &crate::PatternState, _: &()) -> i32 {
                panic!("proof accessed evaluator");
            }
        }
        let position = fixture(CHAIN);
        for (depth, nodes, expected) in [
            (11, 10_000, VcfStatus::ProvenWin { plies: 5 }),
            (4, 10_000, VcfStatus::NotProven),
            (11, 1, VcfStatus::BudgetExceeded),
        ] {
            let mut solver = VcfSolver::new();
            solver.begin_search(nodes);
            let mut board = BoardState::new(&position);
            assert_eq!(
                solver.solve(&mut board, Stone::Black, depth).status,
                expected
            );
            assert_eq!(board, BoardState::new(&position));
            board.assert_consistent();
            solver.begin_search(nodes);
            let mut state = SearchState::new(&position, &InaccessibleEvaluator);
            assert_eq!(
                state
                    .prove_vcf(
                        &mut solver,
                        Stone::Black,
                        depth,
                        &mut SearchBudget::default()
                    )
                    .status,
                expected
            );
            assert_eq!(state.position(), &position);
            assert_eq!(state.key(), board.key());
            assert_eq!(state.patterns(), board.patterns());
        }
    }
}
