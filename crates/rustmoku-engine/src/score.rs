use rustmoku_core::CELL_COUNT;

pub(crate) const MATE_SCORE: i32 = 100_000_000;
pub(crate) const SEARCH_INFINITY: i32 = 200_000_000;

// Static evaluation is clamped to +/-10,000,000. Reserving the top
// CELL_COUNT points below MATE_SCORE cleanly identifies every legal mate score.
pub(crate) const MATE_THRESHOLD: i32 = MATE_SCORE - CELL_COUNT as i32;

pub(crate) fn score_to_tt(score: i32, ply: u8) -> i32 {
    if score >= MATE_THRESHOLD {
        score + i32::from(ply)
    } else if score <= -MATE_THRESHOLD {
        score - i32::from(ply)
    } else {
        score
    }
}

pub(crate) fn score_from_tt(score: i32, ply: u8) -> i32 {
    if score >= MATE_THRESHOLD {
        score - i32::from(ply)
    } else if score <= -MATE_THRESHOLD {
        score + i32::from(ply)
    } else {
        score
    }
}

#[cfg(test)]
mod tests {
    use super::{MATE_SCORE, SEARCH_INFINITY, score_from_tt, score_to_tt};

    #[test]
    fn ordinary_scores_round_trip_unchanged() {
        for score in [-10_000_000, -42, 0, 42, 10_000_000] {
            assert_eq!(score_to_tt(score, 17), score);
            assert_eq!(score_from_tt(score, 17), score);
        }
    }

    #[test]
    fn positive_mate_scores_round_trip() {
        let score = MATE_SCORE - 7;
        assert_eq!(score_from_tt(score_to_tt(score, 7), 7), score);
    }

    #[test]
    fn negative_mate_scores_round_trip() {
        let score = -MATE_SCORE + 7;
        assert_eq!(score_from_tt(score_to_tt(score, 7), 7), score);
    }

    #[test]
    fn mate_score_adjusts_when_probed_at_another_ply() {
        let positive = score_to_tt(MATE_SCORE - 5, 5);
        let negative = score_to_tt(-MATE_SCORE + 5, 5);
        assert_eq!(score_from_tt(positive, 2), MATE_SCORE - 2);
        assert_eq!(score_from_tt(negative, 2), -MATE_SCORE + 2);
    }

    #[test]
    fn normalized_scores_remain_inside_search_infinity() {
        let positive = score_to_tt(MATE_SCORE, u8::MAX);
        let negative = score_to_tt(-MATE_SCORE, u8::MAX);
        assert!(positive < SEARCH_INFINITY);
        assert!(negative > -SEARCH_INFINITY);
        assert_eq!(score_from_tt(positive, u8::MAX), MATE_SCORE);
        assert_eq!(score_from_tt(negative, u8::MAX), -MATE_SCORE);
    }
}
