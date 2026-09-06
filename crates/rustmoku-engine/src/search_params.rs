//! Centralized initial V0.10 selectivity parameters. Evaluation-dependent
//! margins must be retuned when V0.11 replaces the evaluator distribution.

pub(crate) const HISTORY_MAX: i32 = 16_384;
pub(crate) const STRONG_HISTORY: i16 = 1_000;
pub(crate) const LMR_MIN_DEPTH: u8 = 3;
pub(crate) const LMR_MIN_INDEX: usize = 8;
pub(crate) const THREAT_EXTENSION_BUDGET: u8 = 1;
pub(crate) const IIR_MIN_DEPTH: u8 = 7;

pub(crate) fn history_bonus(depth: u8) -> i32 {
    let depth = depth as i32;
    (depth * depth * 8).min(1_024)
}

pub(crate) fn history_malus(depth: u8) -> i32 {
    (history_bonus(depth) / 2).max(1)
}

pub(crate) const fn lmr_base(depth: u8, index: usize) -> u8 {
    let mut reduction = 1;
    if depth >= 7 && index >= 12 {
        reduction += 1;
    }
    if depth >= 10 && index >= 24 {
        reduction += 1;
    }
    reduction
}

pub(crate) const fn lmp_threshold(depth: u8) -> usize {
    match depth {
        1 => 8,
        2 => 14,
        _ => 22,
    }
}

pub(crate) const fn futility_margin(depth: u8) -> i32 {
    600 + 1_200 * depth as i32
}

pub(crate) const fn reverse_futility_margin(depth: u8) -> i32 {
    3_000 + 4_000 * depth as i32
}

pub(crate) const fn razor_margin(depth: u8) -> i32 {
    2_000 + 3_000 * depth as i32
}
