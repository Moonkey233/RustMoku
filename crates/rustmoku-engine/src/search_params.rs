//! Centralized initial V0.10 selectivity parameters. Evaluation-dependent
//! margins must be retuned when V0.11 replaces the evaluator distribution.

pub(crate) const HISTORY_MAX: i32 = 16_384;
pub(crate) const THREAT_EXTENSION_BUDGET: u8 = 1;
#[cfg(test)]
pub(crate) const IIR_MIN_DEPTH: u8 = 7;
