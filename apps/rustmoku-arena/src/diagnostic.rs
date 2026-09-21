//! Arena-only decomposition. No production evaluator or model-format changes.
use rustmoku_core::{Move, Position};
use rustmoku_engine::{
    Evaluator, MixLiteEvaluator, MixLiteState, PatternDelta, PatternEvaluator, PatternState,
    ScoreContract,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Mode {
    #[default]
    Normal,
    ValueOnly,
    PolicyOnly,
}

impl Mode {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "normal" => Ok(Self::Normal),
            "value-only" => Ok(Self::ValueOnly),
            "policy-only" => Ok(Self::PolicyOnly),
            _ => Err("v3-mode must be normal, value-only, or policy-only"),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::ValueOnly => "value-only",
            Self::PolicyOnly => "policy-only",
        }
    }
}

pub(super) struct DiagnosticEvaluator {
    pub model: MixLiteEvaluator,
    pub mode: Mode,
}
impl Evaluator for DiagnosticEvaluator {
    type State = MixLiteState;
    type Undo = ();
    fn supports_analysis_turn(&self) -> bool {
        true
    }
    fn score_contract(&self) -> ScoreContract {
        if self.mode == Mode::PolicyOnly {
            ScoreContract::Pattern
        } else {
            self.model.score_contract()
        }
    }
    fn model_fingerprint(&self) -> Option<[u8; 32]> {
        if self.mode == Mode::Normal {
            return self.model.model_fingerprint();
        }
        let mut hash = Sha256::new();
        hash.update(b"RustMoku-Arena-V3-decomposition-v1");
        hash.update(self.mode.name().as_bytes());
        hash.update(self.model.model_fingerprint()?);
        if self.mode == Mode::PolicyOnly {
            hash.update(PatternEvaluator.model_fingerprint()?);
        }
        Some(hash.finalize().into())
    }
    fn initialize(&self, position: &Position, patterns: &PatternState) -> Self::State {
        self.model.initialize(position, patterns)
    }
    fn make_move(&self, state: &mut Self::State, delta: &PatternDelta) {
        self.model.make_move(state, delta);
    }
    fn unmake_move(&self, state: &mut Self::State, delta: &PatternDelta, undo: ()) {
        self.model.unmake_move(state, delta, undo);
    }
    fn evaluate(&self, position: &Position, patterns: &PatternState, state: &Self::State) -> i32 {
        if self.mode == Mode::PolicyOnly {
            PatternEvaluator.evaluate(position, patterns, &())
        } else {
            self.model.evaluate(position, patterns, state)
        }
    }
    fn policy_score(
        &self,
        position: &Position,
        patterns: &PatternState,
        state: &Self::State,
        at: Move,
    ) -> Option<i32> {
        if self.mode == Mode::ValueOnly {
            None
        } else {
            self.model.policy_score(position, patterns, state, at)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustmoku_engine::{
        AlphaBetaEngine, EngineConfig, MixLiteModel, SearchEngine, SearchLimits,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn fixture() -> MixLiteEvaluator {
        let byte_weights = (65536 + 3) * 32 + 8 * 160;
        let mut bytes = vec![1; 32 + byte_weights + 8 * 4 + (24 + 32 + 8) * 2];
        bytes[..8].copy_from_slice(b"RMLPV003");
        bytes[8..20].copy_from_slice(&[3, 0, 4, 0, 0, 0, 1, 0, 32, 0, 2, 0]);
        for (at, value) in [(20, 16_i32), (24, 64), (28, 500)] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for word in bytes[32 + byte_weights..32 + byte_weights + 32]
            .as_chunks_mut::<4>()
            .0
        {
            word.copy_from_slice(&512_i32.to_le_bytes());
        }
        MixLiteEvaluator::new(Arc::new(
            MixLiteModel::read_from(&mut bytes.as_slice()).unwrap(),
        ))
    }

    struct Audit {
        evaluator: DiagnosticEvaluator,
        restored: Arc<AtomicUsize>,
    }
    impl Evaluator for Audit {
        type State = (Position, MixLiteState);
        type Undo = rustmoku_core::MoveUndo;
        fn initialize(&self, p: &Position, t: &PatternState) -> Self::State {
            (p.clone(), self.evaluator.initialize(p, t))
        }
        fn make_move(&self, s: &mut Self::State, d: &PatternDelta) -> Self::Undo {
            let undo = s.0.make_move(d.played_move()).unwrap();
            self.evaluator.make_move(&mut s.1, d);
            assert_eq!(
                s.1,
                self.evaluator.initialize(&s.0, &PatternState::new(&s.0))
            );
            undo
        }
        fn unmake_move(&self, s: &mut Self::State, d: &PatternDelta, u: Self::Undo) {
            self.evaluator.unmake_move(&mut s.1, d, ());
            s.0.unmake_move(u);
            assert_eq!(
                s.1,
                self.evaluator.initialize(&s.0, &PatternState::new(&s.0))
            );
            self.restored.fetch_add(1, Ordering::Relaxed);
        }
        fn evaluate(&self, p: &Position, t: &PatternState, s: &Self::State) -> i32 {
            self.evaluator.evaluate(p, t, &s.1)
        }
        fn policy_score(
            &self,
            p: &Position,
            t: &PatternState,
            s: &Self::State,
            a: Move,
        ) -> Option<i32> {
            self.evaluator.policy_score(p, t, &s.1, a)
        }
        fn score_contract(&self) -> ScoreContract {
            self.evaluator.score_contract()
        }
    }

    #[test]
    fn decomposition_contracts_and_incremental_roundtrips() {
        let model = fixture();
        let mut p = Position::default();
        p.make_move(Move::CENTER).unwrap();
        let t = PatternState::new(&p);
        let at = Move::from_index(0).unwrap();
        let mut fingerprints = Vec::new();
        for mode in [Mode::Normal, Mode::ValueOnly, Mode::PolicyOnly] {
            let evaluator = DiagnosticEvaluator {
                model: model.clone(),
                mode,
            };
            let state = evaluator.initialize(&p, &t);
            assert_eq!(
                evaluator.evaluate(&p, &t, &state),
                if mode == Mode::PolicyOnly {
                    PatternEvaluator.evaluate(&p, &t, &())
                } else {
                    model.evaluate_position(&p)
                }
            );
            assert_eq!(
                evaluator.policy_score(&p, &t, &state, at),
                if mode == Mode::ValueOnly {
                    None
                } else {
                    model.policy_for(&p, at)
                }
            );
            assert_eq!(
                evaluator.score_contract(),
                if mode == Mode::PolicyOnly {
                    ScoreContract::Pattern
                } else {
                    ScoreContract::RationalV2 { scale: 500 }
                }
            );
            fingerprints.push(evaluator.model_fingerprint());
            let restored = Arc::new(AtomicUsize::new(0));
            let audit = Audit {
                evaluator,
                restored: restored.clone(),
            };
            let mut engine = AlphaBetaEngine::with_config(audit, EngineConfig::default());
            engine.search(&p, SearchLimits::new(2).with_max_nodes(100));
            assert!(restored.load(Ordering::Relaxed) > 1);
        }
        assert_ne!(fingerprints[0], fingerprints[1]);
        assert_ne!(fingerprints[1], fingerprints[2]);
        assert_ne!(fingerprints[0], fingerprints[2]);
        assert!(Mode::parse("invalid").is_err());
        assert!(
            super::super::Options::parse(
                ["--a-v3-mode", "policy-only"].into_iter().map(String::from)
            )
            .is_err()
        );
    }
}
