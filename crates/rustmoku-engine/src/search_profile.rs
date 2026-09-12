//! Immutable, validated score-dependent parameters. Baseline reproduces the
//! historical constants; research flags are independent and default off.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScoreContract {
    #[default]
    Pattern,
    LinearV1,
    RationalV2 {
        scale: i32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchParameters {
    pub aspiration: i32,
    pub futility: [i32; 2],
    pub reverse_futility: [i32; 2],
    pub razor: [i32; 2],
    pub lmp: [u16; 3],
    pub lmr_min_depth: u8,
    pub lmr_min_index: u16,
    pub lmr_stages: [(u8, u16); 2],
    pub iir_min_depth: u8,
    pub strong_history: i16,
    pub history_bonus_factor: u16,
    pub extension_budget: u8,
}

impl SearchParameters {
    pub const BASELINE: Self = Self {
        aspiration: 10_000,
        futility: [600, 1200],
        reverse_futility: [3000, 4000],
        razor: [2000, 3000],
        lmp: [8, 14, 22],
        lmr_min_depth: 3,
        lmr_min_index: 8,
        lmr_stages: [(7, 12), (10, 24)],
        iir_min_depth: 7,
        strong_history: 1000,
        history_bonus_factor: 8,
        extension_budget: 1,
    };

    fn valid(self) -> bool {
        (1..=1_000_000).contains(&self.aspiration)
            && self
                .futility
                .into_iter()
                .chain(self.reverse_futility)
                .chain(self.razor)
                .all(|x| (0..=1_000_000).contains(&x))
            && self.lmp.into_iter().all(|x| (1..=225).contains(&x))
            && (3..=32).contains(&self.lmr_min_depth)
            && (1..=225).contains(&self.lmr_min_index)
            && self
                .lmr_stages
                .into_iter()
                .all(|(depth, index)| (3..=32).contains(&depth) && (1..=225).contains(&index))
            && (3..=32).contains(&self.iir_min_depth)
            && self.strong_history >= 0
            && (1..=32).contains(&self.history_bonus_factor)
            && self.extension_budget <= 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchProfile {
    contract: ScoreContract,
    parameters: SearchParameters,
    policy_lmr: bool,
    singular: bool,
    qsearch_threes: bool,
}

impl SearchProfile {
    #[must_use]
    pub const fn baseline(contract: ScoreContract) -> Self {
        Self {
            contract,
            parameters: SearchParameters::BASELINE,
            policy_lmr: false,
            singular: false,
            qsearch_threes: false,
        }
    }
    pub fn new(
        contract: ScoreContract,
        parameters: SearchParameters,
    ) -> Result<Self, &'static str> {
        if !parameters.valid()
            || matches!(contract, ScoreContract::RationalV2 { scale } if !(1..=10_000_000).contains(&scale))
        {
            return Err("invalid score contract or search parameters");
        }
        Ok(Self {
            parameters,
            ..Self::baseline(contract)
        })
    }
    #[must_use]
    pub const fn with_policy_lmr(mut self, enabled: bool) -> Self {
        self.policy_lmr = enabled;
        self
    }
    #[must_use]
    pub const fn with_singular(mut self, enabled: bool) -> Self {
        self.singular = enabled;
        self
    }
    #[must_use]
    pub const fn contract(self) -> ScoreContract {
        self.contract
    }
    #[must_use]
    pub const fn parameters(self) -> SearchParameters {
        self.parameters
    }
    #[must_use]
    pub const fn policy_lmr(self) -> bool {
        self.policy_lmr
    }
    #[must_use]
    pub const fn singular(self) -> bool {
        self.singular
    }
    #[must_use]
    pub const fn with_qsearch_threes(mut self, enabled: bool) -> Self {
        self.qsearch_threes = enabled;
        self
    }
    #[must_use]
    pub const fn qsearch_threes(self) -> bool {
        self.qsearch_threes
    }
    pub(crate) fn margin(self, coefficients: [i32; 2], depth: u8) -> i32 {
        coefficients[0] + coefficients[1] * i32::from(depth)
    }
}

impl std::fmt::Display for SearchProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (contract, scale) = match self.contract {
            ScoreContract::Pattern => (0, 0),
            ScoreContract::LinearV1 => (1, 0),
            ScoreContract::RationalV2 { scale } => (2, scale),
        };
        let p = self.parameters;
        let v2 = self.qsearch_threes;
        write!(
            f,
            "{},{contract},{scale},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            if v2 { "RMPROFILE2" } else { "RMPROFILE1" },
            p.aspiration,
            p.futility[0],
            p.futility[1],
            p.reverse_futility[0],
            p.reverse_futility[1],
            p.razor[0],
            p.razor[1],
            p.lmp[0],
            p.lmp[1],
            p.lmp[2],
            p.lmr_min_depth,
            p.lmr_min_index,
            p.iir_min_depth,
            p.strong_history,
            p.history_bonus_factor,
            p.extension_budget,
            u8::from(self.policy_lmr),
            u8::from(self.singular),
            p.lmr_stages[0].0,
            p.lmr_stages[0].1,
            p.lmr_stages[1].0,
            p.lmr_stages[1].1
        )?;
        if v2 {
            write!(f, ",{}", u8::from(self.qsearch_threes))
        } else {
            Ok(())
        }
    }
}

impl std::str::FromStr for SearchProfile {
    type Err = &'static str;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() > 1024 {
            return Err("profile exceeds size limit");
        }
        let mut fields = text.trim().split(',');
        let v2 = match fields.next() {
            Some("RMPROFILE1") => false,
            Some("RMPROFILE2") => true,
            _ => return Err("unsupported search profile version"),
        };
        let values: Vec<i32> = fields
            .map(|value| value.parse().map_err(|_| "invalid profile integer"))
            .collect::<Result<_, _>>()?;
        if values.len() != if v2 { 25 } else { 24 } {
            return Err("invalid profile field count");
        }
        let contract = match (values[0], values[1]) {
            (0, 0) => ScoreContract::Pattern,
            (1, 0) => ScoreContract::LinearV1,
            (2, scale) => ScoreContract::RationalV2 { scale },
            _ => return Err("invalid score contract"),
        };
        let small =
            |index: usize| u16::try_from(values[index]).map_err(|_| "profile integer out of range");
        let byte =
            |index: usize| u8::try_from(values[index]).map_err(|_| "profile integer out of range");
        if !matches!(values[18], 0 | 1) || !matches!(values[19], 0 | 1) {
            return Err("invalid profile flags");
        }
        if v2 && values[24..].iter().any(|&value| !matches!(value, 0 | 1)) {
            return Err("invalid research profile flags");
        }
        let parameters = SearchParameters {
            aspiration: values[2],
            futility: [values[3], values[4]],
            reverse_futility: [values[5], values[6]],
            razor: [values[7], values[8]],
            lmp: [small(9)?, small(10)?, small(11)?],
            lmr_stages: [(byte(20)?, small(21)?), (byte(22)?, small(23)?)],
            lmr_min_depth: byte(12)?,
            lmr_min_index: small(13)?,
            iir_min_depth: byte(14)?,
            strong_history: i16::try_from(values[15])
                .map_err(|_| "history threshold out of range")?,
            history_bonus_factor: small(16)?,
            extension_budget: byte(17)?,
        };
        Ok(Self::new(contract, parameters)?
            .with_policy_lmr(values[18] == 1)
            .with_singular(values[19] == 1)
            .with_qsearch_threes(values.get(24) == Some(&1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qsearch_research_profile_roundtrips_without_changing_v1() {
        let baseline = SearchProfile::baseline(ScoreContract::Pattern);
        assert!(baseline.to_string().starts_with("RMPROFILE1,"));
        let profile = baseline.with_qsearch_threes(true);
        assert!(profile.to_string().starts_with("RMPROFILE2,"));
        assert_eq!(
            profile.to_string().parse::<SearchProfile>().unwrap(),
            profile
        );
        assert!(format!("{}2", profile).parse::<SearchProfile>().is_err());
    }
    #[test]
    fn profile_round_trip_rejects_unknown_and_out_of_range_fields() {
        for contract in [
            ScoreContract::Pattern,
            ScoreContract::LinearV1,
            ScoreContract::RationalV2 { scale: 500 },
        ] {
            let profile = SearchProfile::baseline(contract)
                .with_policy_lmr(true)
                .with_singular(true);
            assert_eq!(
                profile.to_string().parse::<SearchProfile>().unwrap(),
                profile
            );
            assert!(format!("{},0", profile).parse::<SearchProfile>().is_err());
            assert!(
                profile
                    .to_string()
                    .replace("10000", "2147483647")
                    .parse::<SearchProfile>()
                    .is_err()
            );
        }
    }
}
