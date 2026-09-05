//! Independent depth-first proof-number implementation.
//!
//! OR uses min(pn)/sum(dn); AND uses sum(pn)/min(dn). The most-proving
//! child's min threshold is capped by second-best + 1; its sum threshold is
//! the parent's threshold minus the other children's contribution. Saturating
//! arithmetic bounds numbers, while only a zero proves/disproves a node.
use rustmoku_core::{CELL_COUNT, Stone};

use super::{
    VctResult, VctStatistics, VctStatus, branches, fact,
    table::{INFINITY, Numbers, Table, TacticalKey},
    threat::ThreatDescriptor,
};
use crate::search_control::{ProofResources, SearchBudget};
use crate::{board_state::BoardState, move_generation::MoveList, principal_variation::PvTable};

#[derive(Clone, Copy, Debug)]
pub(super) enum Exhausted {
    Local,
    Outer,
}

pub(crate) struct VctSolver {
    table: Table,
    remaining_nodes: u64,
    statistics: VctStatistics,
}

impl VctSolver {
    pub(crate) fn new(memory_mib: usize) -> Self {
        Self {
            table: Table::new(memory_mib),
            remaining_nodes: 0,
            statistics: VctStatistics::default(),
        }
    }

    pub(crate) fn begin_search(&mut self, max_nodes: u64) {
        self.table.begin_search();
        self.remaining_nodes = max_nodes;
        self.statistics = VctStatistics::default();
    }

    pub(crate) fn statistics(&self) -> VctStatistics {
        self.statistics
    }

    #[cfg(test)]
    pub(crate) fn solve(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        max_plies: u8,
    ) -> VctResult {
        self.solve_controlled(board, attacker, max_plies, &mut SearchBudget::default())
    }

    pub(crate) fn solve_controlled(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        max_plies: u8,
        budget: &mut SearchBudget,
    ) -> VctResult {
        let available = (CELL_COUNT - board.position().move_count()) as u8;
        let max_plies = max_plies.min(available);
        let mut pv = PvTable::new();
        let outcome = self.canonical(
            board,
            attacker,
            None,
            max_plies,
            0,
            &mut ProofResources {
                pv: &mut pv,
                budget,
            },
        );
        let status = match outcome {
            Ok(Some(plies)) => {
                self.statistics.proven += 1;
                VctStatus::ProvenWin { plies }
            }
            Ok(None) => VctStatus::NoProof,
            Err(Exhausted::Outer) => VctStatus::Interrupted,
            Err(Exhausted::Local) => {
                self.statistics.budget_exhausted += 1;
                VctStatus::BudgetExceeded
            }
        };
        let principal_variation = if matches!(status, VctStatus::ProvenWin { .. }) {
            pv.root_line().to_vec()
        } else {
            Vec::new()
        };
        VctResult {
            status,
            principal_variation,
        }
    }

    fn charge(&mut self, budget: &mut SearchBudget) -> Result<(), Exhausted> {
        if self.remaining_nodes == 0 {
            return Err(Exhausted::Local);
        }
        budget.charge().map_err(|_| Exhausted::Outer)?;
        self.remaining_nodes -= 1;
        self.statistics.nodes += 1;
        Ok(())
    }

    /// Every node inspection costs one, including child initialization, cache
    /// hits, and canonical reconstruction visits. No budget decisions use stats.
    fn inspect(
        &mut self,
        board: &BoardState,
        attacker: Stone,
        active: Option<ThreatDescriptor>,
        depth: u8,
        budget: &mut SearchBudget,
    ) -> Result<Numbers, Exhausted> {
        self.charge(budget)?;
        // Actual immediate facts precede cache and proof-depth cutoffs.
        if let Some(fact) = fact(board, attacker, depth) {
            return Ok(fact.numbers());
        }
        let key = TacticalKey::new(board, attacker, active);
        if let Some(entry) = self.table.probe(key, depth)
            && entry
                .best_move
                .is_none_or(|at| board.position().is_legal(at))
        {
            self.statistics.cache_hits += 1;
            return Ok(entry.numbers);
        }
        Ok(Numbers::UNKNOWN)
    }

    fn dfpn(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        active: Option<ThreatDescriptor>,
        depth: u8,
        threshold: Numbers,
        budget: &mut SearchBudget,
    ) -> Result<Numbers, Exhausted> {
        let mut value = self.inspect(board, attacker, active, depth, budget)?;
        if value.solved() || value.proof >= threshold.proof || value.disproof >= threshold.disproof
        {
            return Ok(value);
        }
        let key = TacticalKey::new(board, attacker, active);
        let is_or = board.position().side_to_move() == attacker;
        let mut moves = MoveList::new();
        for at in branches(board, attacker, active).iter() {
            moves.push(at);
        }
        if moves.is_empty() {
            self.table.store(key, depth, Numbers::NO_PROOF, None, None);
            return Ok(Numbers::NO_PROOF);
        }
        let mut children = [Numbers::UNKNOWN; CELL_COUNT];
        let children = &mut children[..moves.as_slice().len()];
        for (at, child) in moves.iter().zip(children.iter_mut()) {
            let next = if is_or {
                ThreatDescriptor::new(board, at, attacker)
            } else {
                None
            };
            let undo = board.make_move(at).expect("tactical response is legal");
            let result = self.inspect(board, attacker, next, depth - 1, budget);
            board.unmake_move(undo);
            *child = result?;
        }
        value = aggregate(children, is_or);
        let mut best = None;
        while !value.solved()
            && value.proof < threshold.proof
            && value.disproof < threshold.disproof
        {
            let (selected, child_threshold) = select(children, is_or, threshold);
            let at = moves.as_slice()[selected];
            let next = if is_or {
                ThreatDescriptor::new(board, at, attacker)
            } else {
                None
            };
            let undo = board.make_move(at).expect("most-proving child is legal");
            let result = self.dfpn(board, attacker, next, depth - 1, child_threshold, budget);
            board.unmake_move(undo);
            // Interrupted subtrees are never converted into solved disproofs.
            children[selected] = result?;
            best = Some(at);
            value = aggregate(children, is_or);
        }
        self.table.store(key, depth, value, best, None);
        Ok(value)
    }

    /// Iterative parity limits establish the shortest proof at *each* node.
    /// DFPN's first successful branch is not a distance certificate. OR then
    /// chooses the first proven attack at that limit; AND reconstructs every
    /// response at its own shortest limit and selects the longest, ties low.
    /// This work is charged to the same node budget. Only copying the final PV
    /// (and fixed immediate prefixes) is outside expansion accounting.
    pub(super) fn canonical(
        &mut self,
        board: &mut BoardState,
        attacker: Stone,
        active: Option<ThreatDescriptor>,
        cap: u8,
        ply: u8,
        resources: &mut ProofResources<'_>,
    ) -> Result<Option<u8>, Exhausted> {
        self.charge(resources.budget)?;
        resources.pv.clear(ply);
        if let Some(fact) = fact(board, attacker, cap) {
            fact.write_pv(ply, resources.pv);
            return Ok(fact.distance);
        }
        let is_or = board.position().side_to_move() == attacker;
        let key = TacticalKey::new(board, attacker, active);
        let known = self.table.probe(key, cap).and_then(|entry| entry.distance);
        let first = if is_or { 1 } else { 2 };
        let mut shortest = known;
        if shortest.is_none() {
            for depth in (first..=cap).step_by(2) {
                let numbers = self.dfpn(
                    board,
                    attacker,
                    active,
                    depth,
                    Numbers {
                        proof: INFINITY,
                        disproof: INFINITY,
                    },
                    resources.budget,
                )?;
                if numbers.proof == 0 {
                    shortest = Some(depth);
                    break;
                }
                if !numbers.solved() {
                    // Saturated unsolved numbers are still Unknown. With the
                    // practical node limits this numerical ceiling is remote.
                    return Err(Exhausted::Local);
                }
            }
        }
        let Some(depth) = shortest else {
            return Ok(None);
        };
        let mut chosen = None;
        let mut distance = 0;
        for at in branches(board, attacker, active).iter() {
            let next = if is_or {
                ThreatDescriptor::new(board, at, attacker)
            } else {
                None
            };
            let undo = board.make_move(at).expect("certificate move is legal");
            let child = self.canonical(board, attacker, next, depth - 1, ply + 1, resources);
            board.unmake_move(undo);
            match child? {
                Some(child_distance) => {
                    let candidate = child_distance + 1;
                    if chosen.is_none() || candidate > distance {
                        distance = candidate;
                        chosen = Some(at);
                        resources.pv.update(ply, at);
                    }
                    if is_or {
                        break;
                    }
                }
                None if !is_or => {
                    // Reconstruction verifies every AND branch on the actual
                    // board; a missing proof must never emit a partial PV.
                    return Ok(None);
                }
                None => {}
            }
        }
        if chosen.is_none() {
            return Ok(None);
        }
        debug_assert_eq!(distance, depth);
        self.table
            .store(key, cap, Numbers::WIN, chosen, Some(distance));
        Ok(Some(distance))
    }
}

fn sum(values: impl Iterator<Item = u32>) -> u32 {
    values.fold(0_u32, |sum, value| sum.saturating_add(value).min(INFINITY))
}

fn aggregate(children: &[Numbers], is_or: bool) -> Numbers {
    if is_or {
        Numbers {
            proof: children.iter().map(|c| c.proof).min().unwrap_or(INFINITY),
            disproof: sum(children.iter().map(|c| c.disproof)),
        }
    } else {
        Numbers {
            proof: sum(children.iter().map(|c| c.proof)),
            disproof: children
                .iter()
                .map(|c| c.disproof)
                .min()
                .unwrap_or(INFINITY),
        }
    }
}

fn select(children: &[Numbers], is_or: bool, threshold: Numbers) -> (usize, Numbers) {
    let metric = |n: Numbers| if is_or { n.proof } else { n.disproof };
    let selected = (0..children.len())
        .min_by_key(|&i| (metric(children[i]), i))
        .expect("nonempty unsolved node");
    let second = children
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != selected)
        .map(|(_, &n)| metric(n))
        .min()
        .unwrap_or(INFINITY);
    let siblings = sum(children
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != selected)
        .map(|(_, n)| if is_or { n.disproof } else { n.proof }));
    let next = second.saturating_add(1).min(INFINITY);
    let threshold = if is_or {
        Numbers {
            proof: threshold.proof.min(next),
            disproof: threshold.disproof.saturating_sub(siblings).max(1),
        }
    } else {
        Numbers {
            proof: threshold.proof.saturating_sub(siblings).max(1),
            disproof: threshold.disproof.min(next),
        }
    };
    (selected, threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outer_stop_in_dfpn_or_certificate_is_not_a_cached_disproof() {
        let mut position = rustmoku_core::Position::default();
        for index in [110, 0, 111, 14, 82, 210, 97, 224] {
            position
                .make_move(rustmoku_core::Move::from_index(index).unwrap())
                .unwrap();
        }
        let mut board = BoardState::new(&position);
        let mut solver = VctSolver::new(1);
        solver.begin_search(100_000);
        let complete = solver.solve(&mut board, Stone::Black, 5);
        let nodes = solver.statistics().nodes;
        for cap in [1, 20, nodes - 1] {
            solver.begin_search(100_000);
            let mut budget = SearchBudget::new(
                crate::SearchLimits::new(1).with_max_nodes(cap),
                crate::CancellationToken::new(),
            );
            let result = solver.solve_controlled(&mut board, Stone::Black, 5, &mut budget);
            assert_eq!(result.status, VctStatus::Interrupted);
            assert!(result.principal_variation.is_empty());
            assert_eq!(solver.statistics().budget_exhausted, 0);
            assert_eq!(board, BoardState::new(&position));
            assert!(
                solver
                    .table
                    .probe(TacticalKey::new(&board, Stone::Black, None), 5)
                    .is_none_or(|entry| entry.numbers.disproof != 0)
            );
            let resumed = solver.solve(&mut board, Stone::Black, 5);
            assert_eq!(resumed.status, complete.status);
            assert_eq!(resumed.principal_variation, complete.principal_variation);
        }
    }

    #[test]
    fn proof_numbers_thresholds_and_saturation_follow_and_or_semantics() {
        let children = [
            Numbers {
                proof: 2,
                disproof: 7,
            },
            Numbers {
                proof: 4,
                disproof: 3,
            },
            Numbers {
                proof: 2,
                disproof: 8,
            },
        ];
        let threshold = Numbers {
            proof: 20,
            disproof: 30,
        };
        assert_eq!(
            aggregate(&children, true),
            Numbers {
                proof: 2,
                disproof: 18
            }
        );
        assert_eq!(
            aggregate(&children, false),
            Numbers {
                proof: 8,
                disproof: 3
            }
        );
        assert_eq!(
            select(&children, true, threshold),
            (
                0,
                Numbers {
                    proof: 3,
                    disproof: 19
                }
            )
        );
        assert_eq!(
            select(&children, false, threshold),
            (
                1,
                Numbers {
                    proof: 16,
                    disproof: 8
                }
            )
        );
        assert_eq!(
            aggregate(&[Numbers::WIN, Numbers::NO_PROOF], true),
            Numbers::WIN
        );
        assert_eq!(
            aggregate(&[Numbers::WIN, Numbers::NO_PROOF], false),
            Numbers::NO_PROOF
        );
        assert_eq!(sum([INFINITY, INFINITY].into_iter()), INFINITY);
    }

    #[test]
    fn interrupted_dfpn_and_certificate_are_unknown_and_restore_every_board_field() {
        let mut position = rustmoku_core::Position::default();
        for index in [110, 0, 111, 14, 82, 210, 97, 224] {
            position
                .make_move(rustmoku_core::Move::from_index(index).unwrap())
                .unwrap();
        }
        let mut solver = VctSolver::new(1);
        let mut board = BoardState::new(&position);
        solver.begin_search(100_000);
        let complete = solver.solve(&mut board, Stone::Black, 5);
        assert_eq!(complete.status, VctStatus::ProvenWin { plies: 5 });
        let nodes = solver.statistics().nodes;
        // Includes interruption in initial expansion and late certificate work.
        for budget in [0, 1, 20, nodes - 1] {
            solver.begin_search(budget);
            let result = solver.solve(&mut board, Stone::Black, 5);
            assert_eq!(result.status, VctStatus::BudgetExceeded);
            assert!(result.principal_variation.is_empty());
            assert_eq!(solver.statistics().nodes, budget);
            assert_eq!(board, BoardState::new(&position));
            board.assert_consistent();
            let key = TacticalKey::new(&board, Stone::Black, None);
            assert!(
                solver
                    .table
                    .probe(key, 5)
                    .is_none_or(|entry| entry.numbers.disproof != 0)
            );
            // Replenish in the same generation to catch false cached disproofs.
            solver.remaining_nodes = 100_000;
            assert_eq!(
                solver.solve(&mut board, Stone::Black, 5).status,
                complete.status
            );
        }
        // With a one-bucket cache, eviction must only cost work, never proof truth.
        let mut tiny = VctSolver::new(0);
        tiny.begin_search(100_000);
        let evicted = tiny.solve(&mut board, Stone::Black, 5);
        assert_eq!(evicted.status, complete.status);
        assert_eq!(evicted.principal_variation, complete.principal_variation);
    }
}
