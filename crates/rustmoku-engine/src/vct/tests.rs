use super::*;
use crate::pattern::ThreatProfile;
use rustmoku_core::Position;

const DOUBLE_THREE: &[usize] = &[110, 0, 111, 14, 82, 210, 97, 224];
const OPEN_THREE: &[usize] = &[110, 0, 111, 224];

fn solve(position: &Position, depth: u8, nodes: u64) -> VctResult {
    let mut solver = VctSolver::new(1);
    solver.begin_search(nodes);
    let mut board = BoardState::new(position);
    let result = solver.solve(&mut board, position.side_to_move(), depth);
    assert_eq!(board, BoardState::new(position));
    result
}

fn verify(position: &Position, result: &VctResult, distance: u8) {
    assert_eq!(result.status, VctStatus::ProvenWin { plies: distance });
    assert_eq!(result.principal_variation.len(), usize::from(distance));
    let mut replay = position.clone();
    for &at in &result.principal_variation {
        replay.make_move(at).unwrap();
    }
    assert_eq!(replay.winner(), Some(position.side_to_move()));
}

#[test]
fn dfpn_agrees_with_all_legal_reference_on_shallow_proofs_and_refutations() {
    for (indices, depth) in [
        (OPEN_THREE, 5),
        (DOUBLE_THREE, 3),
        (DOUBLE_THREE, 5),
        (&[110, 0, 111, 14, 112, 224][..], 3),
    ] {
        let position = fixture(indices);
        let expected = reference(&mut BoardState::new(&position), Stone::Black, depth);
        let result = solve(&position, depth, 100_000);
        match expected {
            Some(expected) => {
                verify(&position, &result, expected.distance);
                assert_eq!(result.principal_variation, expected.pv);
            }
            None => assert_eq!(result.status, VctStatus::NoProof),
        }
    }
}

#[test]
fn generated_tactical_states_agree_with_all_legal_defender_oracle() {
    // A bounded deterministic generator, not a random-game corpus: translate,
    // reflect and rotate forcing seeds, then add legal nearby/distant stones.
    // Both colors attack, and shallow caps exercise proofs and refutations.
    let seeds = [OPEN_THREE, DOUBLE_THREE, &[110, 0, 111, 14, 112, 224][..]];
    let mut tested = 0;
    let mut proven = 0;
    for case in 0..48 {
        let mut position = Position::default();
        if case % 2 != 0 {
            position.make_move(at(32)).unwrap();
        }
        for &index in seeds[case % seeds.len()] {
            let (mut row, mut col) = (index / 15, index % 15);
            for _ in 0..(case / 3) % 4 {
                (row, col) = (col, 14 - row);
            }
            if case / 12 % 2 != 0 {
                col = 14 - col;
            }
            // Shifting modulo the board also supplies boundary/broken shapes.
            row = (row + case / 24) % 15;
            position.make_move(at(row * 15 + col)).unwrap();
        }
        for offset in 0..2 * (case / 12) {
            let start = (case * 37 + offset * 61) % 225;
            let next = (0..225)
                .map(|i| at((start + i) % 225))
                .find(|&m| position.is_legal(m) && !position.would_win(m, position.side_to_move()))
                .unwrap();
            position.make_move(next).unwrap();
        }
        let mut board = BoardState::new(&position);
        let attacker = position.side_to_move();
        if attacks(board.patterns(), attacker).is_empty() {
            continue;
        }
        let depth = if case / 3 % 2 == 0 { 3 } else { 5 };
        let expected = reference(&mut board, attacker, depth);
        assert_eq!(board, BoardState::new(&position));
        let result = solve(&position, depth, 500_000);
        match expected {
            Some(expected) => {
                proven += 1;
                verify(&position, &result, expected.distance);
                assert_eq!(result.principal_variation, expected.pv, "case {case}");
            }
            None => assert_eq!(result.status, VctStatus::NoProof, "case {case}"),
        }
        tested += 1;
    }
    assert!(
        tested >= 40,
        "insufficient filtered tactical states: {tested}"
    );
    assert!(
        proven >= 8 && proven < tested,
        "audit must cover both outcomes"
    );
    eprintln!(
        "VCT audit: {tested} tactical states, {proven} proven, {} refuted",
        tested - proven
    );
}

#[test]
fn open_three_responses_are_audited_against_every_legal_move() {
    // Includes edge and compound witnesses. Omitted moves must preserve the
    // complete witness and leave no immediate defender winning point; playing
    // an actual continuation must then produce two attacker winning cells.
    for (indices, gain) in [
        (OPEN_THREE, 112),
        (DOUBLE_THREE, 112),
        (&[1, 210, 2, 224][..], 3),
    ] {
        let position = fixture(indices);
        let mut board = BoardState::new(&position);
        let descriptor = ThreatDescriptor::new(&board, at(gain), Stone::Black).unwrap();
        let gain_undo = board.make_move(at(gain)).unwrap();
        let responses = descriptor.responses(&board, Stone::Black);
        assert!(descriptor.defenses.iter().count() >= 3);
        for reply in Move::all()
            .filter(|&reply| board.position().is_legal(reply))
            .collect::<Vec<_>>()
        {
            if responses.test(reply) {
                continue;
            }
            assert!(!descriptor.dependencies.test(reply));
            let undo = board.make_move(reply).unwrap();
            assert!(board.patterns().winning_moves(Stone::White).is_empty());
            let mut preserved = false;
            for continuation in descriptor.continuations.iter() {
                if !board.position().is_legal(continuation) {
                    continue;
                }
                let attack = board.make_move(continuation).unwrap();
                preserved |= board.patterns().winning_moves(Stone::Black).iter().count() >= 2;
                board.unmake_move(attack);
            }
            assert!(preserved, "omitted reply {}", reply.index());
            board.unmake_move(undo);
        }
        board.unmake_move(gain_undo);
        assert_eq!(board, BoardState::new(&position));
    }
}

#[test]
fn one_direct_defense_refutes_an_apparent_open_three_attack() {
    let position = fixture(OPEN_THREE);
    let mut board = BoardState::new(&position);
    let descriptor = ThreatDescriptor::new(&board, at(112), Stone::Black).unwrap();
    let gain = board.make_move(at(112)).unwrap();
    let responses = descriptor.responses(&board, Stone::Black);
    let mut wins = 0;
    let mut refutations = 0;
    for reply in responses.iter() {
        let undo = board.make_move(reply).unwrap();
        if reference(&mut board, Stone::Black, 3).is_some() {
            wins += 1;
        } else {
            refutations += 1;
        }
        board.unmake_move(undo);
    }
    assert!(wins > 0 && refutations > 0);
    assert!(reference(&mut board, Stone::Black, 4).is_none());
    board.unmake_move(gain);
    assert_eq!(solve(&position, 5, 20_000).status, VctStatus::NoProof);
}

#[test]
fn defender_four_counter_threat_outside_costs_is_searched_and_can_refute() {
    let position = fixture(&[110, 16, 111, 17, 82, 18, 97, 224]);
    let mut board = BoardState::new(&position);
    let descriptor = ThreatDescriptor::new(&board, at(112), Stone::Black).unwrap();
    assert_eq!(descriptor.kind, ThreatProfile::DoubleThree);
    let gain = board.make_move(at(112)).unwrap();
    let counter = at(19);
    assert!(!descriptor.defenses.test(counter));
    assert!(descriptor.responses(&board, Stone::Black).test(counter));
    let defense = board.make_move(counter).unwrap();
    assert!(board.patterns().winning_moves(Stone::White).iter().count() >= 2);
    assert!(reference(&mut board, Stone::Black, 3).is_none());
    board.unmake_move(defense);
    assert!(reference(&mut board, Stone::Black, 4).is_none());
    board.unmake_move(gain);
    assert_eq!(solve(&position, 5, 100_000).status, VctStatus::NoProof);
}

#[test]
fn compound_and_node_requires_every_defense_and_replays_canonical_terminal_pv() {
    let position = fixture(DOUBLE_THREE);
    let result = solve(&position, 5, 100_000);
    verify(&position, &result, 5);
    assert_eq!(result.principal_variation[0], at(112));
    let mut board = BoardState::new(&position);
    let descriptor = ThreatDescriptor::new(&board, at(112), Stone::Black).unwrap();
    assert_eq!(descriptor.kind, ThreatProfile::DoubleThree);
    let gain = board.make_move(at(112)).unwrap();
    let responses = descriptor.responses(&board, Stone::Black);
    assert!(responses.iter().count() > 4);
    for reply in responses.iter() {
        let undo = board.make_move(reply).unwrap();
        assert_eq!(reference(&mut board, Stone::Black, 3).unwrap().distance, 3);
        board.unmake_move(undo);
    }
    board.unmake_move(gain);
    board.assert_consistent();
}

#[test]
fn attacker_minimizes_and_defender_maximizes_actual_proof_distance() {
    let position = fixture(&[
        108, 107, 109, 14, 110, 210, 66, 224, 81, 195, 170, 0, 171, 2,
    ]);
    let mut board = BoardState::new(&position);
    let descriptor = ThreatDescriptor::new(&board, at(172), Stone::Black).unwrap();
    assert_eq!(descriptor.kind, ThreatProfile::OpenThree);
    let gain = board.make_move(at(172)).unwrap();
    let mut solver = VctSolver::new(1);
    solver.begin_search(500_000);
    let mut pv = PvTable::new();
    let distance = solver
        .canonical(
            &mut board,
            Stone::Black,
            Some(descriptor),
            6,
            0,
            &mut crate::search_control::ProofResources {
                pv: &mut pv,
                budget: &mut crate::search_control::SearchBudget::default(),
            },
        )
        .unwrap();
    assert_eq!(distance, Some(6));
    let slowest = pv.root_line()[0];
    let mut min = u8::MAX;
    let mut max = 0;
    let mut canonical = None;
    for reply in descriptor.responses(&board, Stone::Black).iter() {
        let undo = board.make_move(reply).unwrap();
        let result = solver.solve(&mut board, Stone::Black, 5);
        let VctStatus::ProvenWin { plies } = result.status else {
            panic!("defense did not retain proof");
        };
        min = min.min(plies);
        if plies > max {
            max = plies;
            canonical = Some(reply);
        }
        board.unmake_move(undo);
    }
    assert_eq!((min, max), (3, 5));
    assert_eq!(Some(slowest), canonical);
    board.unmake_move(gain);
    assert_eq!(board, BoardState::new(&position));

    // A smaller-index five-ply attack must lose to a faster three-ply attack.
    let shorter = fixture(&[
        108, 107, 109, 0, 110, 2, 66, 4, 81, 6, 171, 20, 172, 22, 173, 24,
    ]);
    let result = solve(&shorter, 7, 100_000);
    verify(&shorter, &result, 3);
    assert_eq!(result.principal_variation[0], at(170));
}

#[test]
fn root_integration_is_gated_deterministic_and_emits_complete_vct_metadata() {
    use crate::{
        AlphaBetaEngine, EngineConfig, PatternEvaluator, SearchEngine, SearchLimits,
        TacticalProofKind,
    };
    let position = fixture(DOUBLE_THREE);
    let mut engine = AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1));
    let first = engine.search(&position, SearchLimits::new(2));
    assert_eq!(first.tactical_proof.unwrap().kind, TacticalProofKind::Vct);
    assert_eq!(first.tactical_proof.unwrap().plies, 5);
    assert_eq!((first.completed_depth, first.seldepth), (0, 5));
    assert_eq!(first.score, crate::score::MATE_SCORE - 5);
    assert_eq!(first.statistics.vcf_proven, 0);
    assert_eq!(first.statistics.vct_proven, 1);
    assert_eq!(first.statistics.nodes, 0);
    let quiet = engine.search(&Position::default(), SearchLimits::new(2));
    assert_eq!(quiet.statistics.vct_nodes, 0);
    let repeated = engine.search(&position, SearchLimits::new(2));
    assert_eq!(first, repeated);
    assert_eq!(position, fixture(DOUBLE_THREE));
    let fallback =
        AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1).with_vct_limits(5, 1))
            .search(&position, SearchLimits::new(1));
    assert_eq!(fallback.statistics.vct_budget_exhausted, 1);
    assert_eq!(fallback.completed_depth, 1);
    assert!(fallback.tactical_proof.is_none());
    let disabled =
        AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1).with_vct_limits(0, 0))
            .search(&position, SearchLimits::new(1));
    assert_eq!(disabled.statistics.vct_nodes, 0);
}

#[test]
fn exact_immediate_facts_precede_active_threats_and_depth_caps() {
    let position = fixture(&[110, 1, 111, 2, 112, 3, 0, 4]);
    let mut board = BoardState::new(&position);
    let descriptor = ThreatDescriptor::new(&board, at(109), Stone::Black).unwrap();
    assert_eq!(descriptor.kind, ThreatProfile::OpenFour);
    let gain = board.make_move(at(109)).unwrap();
    assert_eq!(
        board.patterns().winning_moves(Stone::Black).iter().count(),
        2
    );
    for depth in [0, 2, 10] {
        assert_eq!(
            fact(&board, Stone::Black, depth).unwrap().numbers(),
            Numbers::NO_PROOF
        );
    }
    board.unmake_move(gain);
    assert_eq!(board, BoardState::new(&position));
    // With no defender counter-win, the same immediate double points prove two
    // plies, even when there is no inherited non-immediate threat descriptor.
    let quiet = fixture(&[110, 0, 111, 14, 112, 224]);
    let mut board = BoardState::new(&quiet);
    let gain = board.make_move(at(109)).unwrap();
    let mut solver = VctSolver::new(0);
    solver.begin_search(20);
    let result = solver.solve(&mut board, Stone::Black, 2);
    assert_eq!(result.status, VctStatus::ProvenWin { plies: 2 });
    let defense = board.make_move(result.principal_variation[0]).unwrap();
    let win = board.make_move(result.principal_variation[1]).unwrap();
    assert_eq!(board.position().winner(), Some(Stone::Black));
    assert_eq!(
        solver.solve(&mut board, Stone::Black, 0).status,
        VctStatus::ProvenWin { plies: 0 }
    );
    board.unmake_move(win);
    board.unmake_move(defense);
    board.unmake_move(gain);
    assert_eq!(board, BoardState::new(&quiet));
}

fn at(index: usize) -> Move {
    Move::from_index(index).unwrap()
}
fn fixture(indices: &[usize]) -> Position {
    let mut position = Position::default();
    for &index in indices {
        position.make_move(at(index)).unwrap();
    }
    position
}

#[derive(Debug, PartialEq, Eq)]
struct ReferenceProof {
    distance: u8,
    pv: Vec<Move>,
}

/// Deliberately small independent minimax oracle: Core tests immediate wins,
/// every legal defender move is enumerated, and no tactical table is consulted.
/// Only the attack vocabulary is shared with production. No production response
/// generator, DFPN recurrence, thresholds, or certificate logic is used.
fn reference(board: &mut BoardState, attacker: Stone, depth: u8) -> Option<ReferenceProof> {
    if let Some(winner) = board.position().winner() {
        return (winner == attacker).then_some(ReferenceProof {
            distance: 0,
            pv: vec![],
        });
    }
    if depth == 0 || board.position().is_full() {
        return None;
    }
    let side = board.position().side_to_move();
    let wins: Vec<_> = Move::all()
        .filter(|&at| board.position().would_win(at, side))
        .collect();
    if let Some(&at) = wins.first() {
        return (side == attacker).then_some(ReferenceProof {
            distance: 1,
            pv: vec![at],
        });
    }
    let opponent_wins: Vec<_> = Move::all()
        .filter(|&at| board.position().would_win(at, side.opponent()))
        .collect();
    if opponent_wins.len() >= 2 {
        if side == attacker || depth < 2 {
            return None;
        }
        // Exact known losses prefer canonical resistance at actual threat
        // points, matching the public tactical policy without sharing its code.
        let at = opponent_wins[0];
        let reply = opponent_wins[1];
        return Some(ReferenceProof {
            distance: 2,
            pv: vec![at, reply],
        });
    }
    // Without a winning point, the attacker needs at least another attack and
    // reply before the final win. This exact floor keeps the shallow oracle tiny.
    if (side == attacker && depth < 3)
        || (side != attacker && opponent_wins.is_empty() && depth < 4)
    {
        return None;
    }
    let mut best: Option<ReferenceProof> = None;
    for at in Move::all() {
        if !board.position().is_legal(at) {
            continue;
        }
        if side == attacker
            && (board.patterns().profile(at, attacker) < ThreatProfile::OpenThree
                || opponent_wins.first().is_some_and(|&win| win != at))
        {
            continue;
        }
        let undo = board.make_move(at).unwrap();
        let child = reference(board, attacker, depth - 1);
        board.unmake_move(undo);
        match child {
            None if side != attacker => return None,
            Some(mut proof) => {
                proof.distance += 1;
                proof.pv.insert(0, at);
                let replace = best.as_ref().is_none_or(|current| {
                    if side == attacker {
                        proof.distance < current.distance
                    } else {
                        proof.distance > current.distance
                    }
                });
                if replace {
                    best = Some(proof);
                }
                // Own immediate wins were already excluded, so three is the
                // exact lower distance bound. Ascending attacks make this first
                // three-ply proof canonical; later attacks cannot improve it.
                // Every AND reply in this certificate was still enumerated.
                if side == attacker && best.as_ref().is_some_and(|proof| proof.distance == 3) {
                    return best;
                }
            }
            None => {}
        }
    }
    best
}
