//! Offline label traversal exists only on independently verified books.

use super::*;

pub struct ProofTrainingSample<'a> {
    /// Legal replay including this sample's position; owned by the traversal.
    pub game: &'a Game,
    pub lineage: CanonicalPositionKey,
    pub attacker: Stone,
    /// Side-to-move outcome. Unknown positions never reach this interface.
    pub value: i8,
    /// A proven attacker action, never an arbitrary defender obligation.
    pub policy: Option<Move>,
    pub distance: ProofDistance,
}

impl VerifiedProofBook {
    pub fn visit_training_positions(
        &self,
        max_positions: usize,
        mut visitor: impl FnMut(ProofTrainingSample<'_>),
    ) -> Result<usize, ProofBookError> {
        let mut count = 0;
        for root in &self.roots {
            let mut game = Game::new(RuleSet::Freestyle);
            for &at in &root.moves {
                game.play_move(at).expect("verified root replay");
            }
            let mut traversal = TrainingTraversal {
                book: self,
                root,
                seen: BTreeSet::new(),
                count: &mut count,
                max_positions,
                visitor: &mut visitor,
            };
            traversal.visit(&mut game)?;
        }
        Ok(count)
    }
}

struct TrainingTraversal<'a, F> {
    book: &'a VerifiedProofBook,
    root: &'a StoredRoot,
    seen: BTreeSet<CanonicalPositionKey>,
    count: &'a mut usize,
    max_positions: usize,
    visitor: &'a mut F,
}

impl<F: FnMut(ProofTrainingSample<'_>)> TrainingTraversal<'_, F> {
    fn visit(&mut self, game: &mut Game) -> Result<(), ProofBookError> {
        let canonical = CanonicalPosition::new(game.position());
        if !self.seen.insert(canonical.key()) {
            return Ok(());
        }
        if *self.count >= self.max_positions {
            return Err(ProofBookError::VerificationLimit(
                "proof label export limit",
            ));
        }
        let entry = if game.position().winner().is_some() {
            None
        } else {
            let key = EntryKey {
                attacker: self.root.attacker.into(),
                position: canonical.key(),
            };
            let index = self
                .book
                .entries
                .binary_search_by_key(&key, |entry| entry.key)
                .expect("verified strategy entry");
            Some(self.book.entries[index])
        };
        let policy = entry.and_then(|entry| {
            if game.position().side_to_move() != self.root.attacker {
                return None;
            }
            match entry.action {
                StoredAction::AttackerMove(at) => Some(canonical.move_to_original(at)),
                StoredAction::Vcf { best_move, .. } | StoredAction::Vct { best_move, .. } => {
                    best_move.map(|at| canonical.move_to_original(at))
                }
                _ => None,
            }
        });
        *self.count += 1;
        (self.visitor)(ProofTrainingSample {
            game,
            lineage: self.root.key,
            attacker: self.root.attacker,
            value: if game.position().side_to_move() == self.root.attacker {
                1
            } else {
                -1
            },
            policy,
            distance: entry.map_or(ProofDistance::Exact(0), |entry| entry.distance),
        });
        match entry.map(|entry| entry.action) {
            Some(StoredAction::AttackerMove(_)) => {
                game.play_move(policy.expect("verified attacker policy"))
                    .expect("verified legal action");
                let result = self.visit(game);
                game.undo().expect("export transition undo");
                result?;
            }
            Some(StoredAction::DefenderAll) => {
                for at in Move::all() {
                    if game.position().is_legal(at) {
                        game.play_move(at).expect("checked defender move");
                        let result = self.visit(game);
                        game.undo().expect("export transition undo");
                        result?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CancellationToken, OfflineSolver, ProofOutcome, SolverLimits};

    #[test]
    fn verified_labels_keep_perspective_policy_and_resource_limits() {
        for moves in ["H8 A1 I8 A2 J8 B1 K8 B2", "H8 A1 I8 A2 J8 B1 K8"] {
            let game = Game::from_record(&format!("RustMoku 1\nrules=freestyle\nmoves={moves}\n"))
                .unwrap();
            let mut solver = OfflineSolver::new(&game, Stone::Black).unwrap();
            assert_eq!(
                solver.solve(SolverLimits::new(100)).outcome,
                ProofOutcome::ProvenWin
            );
            let book = solver.export_proof_book().unwrap();
            assert!(
                book.clone()
                    .verify_with_limits(ProofBookVerifyLimits::default().with_total_work(0))
                    .is_err()
            );
            assert!(
                book.clone()
                    .verify_with_limits(ProofBookVerifyLimits::default().with_time(Duration::ZERO))
                    .is_err()
            );
            let cancellation = CancellationToken::new();
            cancellation.cancel();
            assert!(
                book.clone()
                    .verify_controlled(ProofBookVerifyLimits::default(), cancellation)
                    .is_err()
            );
            let verified = book.verify().unwrap();
            assert!(
                verified
                    .visit_training_positions(0, |_| panic!("zero cap"))
                    .is_err()
            );
            let mut samples = 0;
            verified
                .visit_training_positions(1000, |sample| {
                    samples += 1;
                    let side = sample.game.position().side_to_move();
                    assert_eq!(sample.value, if side == sample.attacker { 1 } else { -1 });
                    if side != sample.attacker {
                        assert_eq!(sample.policy, None);
                    }
                    if let Some(at) = sample.policy {
                        assert!(sample.game.position().is_legal(at));
                    }
                    let mut replay = Game::new(RuleSet::Freestyle);
                    for at in sample.game.history() {
                        replay.play_move(at).unwrap();
                    }
                    assert_eq!(replay.position(), sample.game.position());
                })
                .unwrap();
            assert!(samples > 0);
        }
    }
}
