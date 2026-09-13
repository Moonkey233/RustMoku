use super::*;

/// Which directions of the returned nominal-depth score are verified. Negamax
/// swaps these directions; a cutoff needs only one child's lower bound, whereas
/// an upper bound needs every relevant child's upper bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BoundValidity {
    pub(super) lower: bool,
    pub(super) upper: bool,
}

impl BoundValidity {
    pub(super) const UNVERIFIED: Self = Self {
        lower: false,
        upper: false,
    };
    pub(super) const VERIFIED: Self = Self {
        lower: true,
        upper: true,
    };

    pub(super) fn include(&mut self, child: Self, score: i32, best_score: i32) {
        self.upper &= child.upper;
        if score > best_score {
            self.lower = child.lower;
        } else if score == best_score {
            self.lower |= child.lower;
        }
    }

    pub(super) fn supports(self, bound: Bound) -> bool {
        match bound {
            Bound::Lower => self.lower,
            Bound::Upper => self.upper,
            Bound::Exact => self.lower && self.upper,
            Bound::Empty => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct NodeResult {
    pub(super) score: i32,
    pub(super) validity: BoundValidity,
}

impl NodeResult {
    pub(super) fn unverified(score: i32) -> Self {
        Self {
            score,
            validity: BoundValidity::UNVERIFIED,
        }
    }

    pub(super) fn verified(score: i32, alpha: i32, beta: i32) -> Self {
        let bound = classify_bound(score, alpha, beta);
        Self {
            score,
            validity: BoundValidity {
                lower: bound != Bound::Upper,
                upper: bound != Bound::Lower,
            },
        }
    }

    pub(super) fn complete(score: i32) -> Self {
        Self {
            score,
            validity: BoundValidity::VERIFIED,
        }
    }
}

impl std::ops::Neg for NodeResult {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            score: -self.score,
            validity: BoundValidity {
                lower: self.validity.upper,
                upper: self.validity.lower,
            },
        }
    }
}
