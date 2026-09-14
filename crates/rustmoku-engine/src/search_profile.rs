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
    research: ResearchParameters,
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
            research: ResearchParameters {
                normalized_scores: matches!(contract, ScoreContract::RationalV2 { .. }),
                ..ResearchParameters::OFF
            },
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
    pub const fn research(self) -> ResearchParameters {
        self.research
    }
    pub fn with_research(mut self, parameters: ResearchParameters) -> Result<Self, &'static str> {
        if !parameters.valid() {
            return Err("invalid research parameters");
        }
        self.research = parameters;
        Ok(self)
    }
    /// Named validated input surface for offline parameter search. No tuning runs here.
    pub fn tuning_parameters(self) -> impl Iterator<Item = (&'static str, i32)> {
        self.research.fields().into_iter()
    }
    pub fn raw_threshold(self, reference: i32) -> i32 {
        if self.research.normalized_scores {
            self.contract.raw_threshold(reference)
        } else {
            reference
        }
    }
    pub fn reference_score(self, raw: i32) -> i32 {
        if self.research.normalized_scores {
            self.contract.reference_score(raw)
        } else {
            raw
        }
    }
    pub(crate) fn margin(self, coefficients: [i32; 2], depth: u8) -> i32 {
        self.raw_threshold(coefficients[0] + coefficients[1] * i32::from(depth))
    }
}

impl std::fmt::Display for SearchProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.research != ResearchParameters::OFF {
            let mut legacy = *self;
            legacy.research = ResearchParameters::OFF;
            write!(f, "RMPROFILE3;base={legacy}")?;
            for (key, value) in self.research.fields() {
                write!(f, ";{key}={value}")?;
            }
            return Ok(());
        }
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
        if let Some(named) = text.trim().strip_prefix("RMPROFILE3;") {
            let mut fields = named.split(';');
            let base = fields
                .next()
                .and_then(|field| field.strip_prefix("base="))
                .ok_or("missing legacy base")?;
            if !base.starts_with("RMPROFILE1,") && !base.starts_with("RMPROFILE2,") {
                return Err("invalid legacy base");
            }
            let mut values = ResearchParameters::OFF.fields();
            let mut seen = 0u32;
            for field in fields {
                let (key, value) = field.split_once('=').ok_or("invalid named parameter")?;
                let index = values
                    .iter()
                    .position(|(name, _)| *name == key)
                    .ok_or("unknown named parameter")?;
                if seen & (1 << index) != 0 {
                    return Err("duplicate named parameter");
                }
                seen |= 1 << index;
                values[index].1 = value.parse().map_err(|_| "invalid named integer")?;
            }
            // The new policy switch is optional when reading old named profiles.
            if seen | (1 << (values.len() - 1)) != (1 << values.len()) - 1 {
                return Err("missing named parameter");
            }
            return base
                .parse::<Self>()?
                .with_research(ResearchParameters::from_fields(values)?);
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
        Self::new(contract, parameters)?
            .with_policy_lmr(values[18] == 1)
            .with_singular(values[19] == 1)
            .with_qsearch_threes(values.get(24) == Some(&1))
            .with_research(ResearchParameters::OFF)
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

/// Named experimental schema. Defaults preserve existing search behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResearchParameters {
    pub lmr_v2: bool,
    /// Independent ablation; true preserves legacy V2 profiles that coupled policy.
    pub lmr_v2_policy: bool,
    pub lmr_divisor: u16,
    pub lmr_cut_bonus: u8,
    pub improving: bool,
    pub improving_lmr_discount: u8,
    pub improving_margin_percent: u16,
    pub improving_lmp_bonus: u16,
    pub iid: bool,
    pub iid_min_depth: u8,
    pub iid_reduction: u8,
    pub policy_pruning: bool,
    pub policy_max_depth: u8,
    pub policy_tail_percent: u8,
    pub competitive_tt: bool,
    pub time_stability: u16,
    pub time_drop: u16,
    pub growth_min_q8: u16,
    pub growth_max_q8: u16,
    pub growth_initial_q8: u16,
    pub growth_ema_weight: u16,
    pub normalized_scores: bool,
    pub null_move: bool,
    pub null_min_depth: u8,
    pub null_reduction: u8,
}
impl ResearchParameters {
    pub const OFF: Self = Self {
        lmr_v2: false,
        lmr_v2_policy: true,
        lmr_divisor: 4,
        lmr_cut_bonus: 1,
        improving: false,
        improving_lmr_discount: 1,
        improving_margin_percent: 125,
        improving_lmp_bonus: 4,
        iid: false,
        iid_min_depth: 6,
        iid_reduction: 3,
        policy_pruning: false,
        policy_max_depth: 2,
        policy_tail_percent: 10,
        competitive_tt: false,
        time_stability: 250,
        time_drop: 10000,
        growth_min_q8: 64,
        growth_max_q8: 2048,
        growth_initial_q8: 384,
        growth_ema_weight: 3,
        normalized_scores: false,
        null_move: false,
        null_min_depth: 6,
        null_reduction: 3,
    };
    fn valid(self) -> bool {
        self.time_stability > 0
            && self.time_drop > self.time_stability
            && (16..=256).contains(&self.growth_min_q8)
            && (256..=4096).contains(&self.growth_max_q8)
            && (self.growth_min_q8..=self.growth_max_q8).contains(&self.growth_initial_q8)
            && (1..=7).contains(&self.growth_ema_weight)
            && (4..=32).contains(&self.null_min_depth)
            && (2..=8).contains(&self.null_reduction)
            && self.null_reduction < self.null_min_depth
            && (2..=32).contains(&self.lmr_divisor)
            && self.lmr_cut_bonus <= 2
            && self.improving_lmr_discount <= 2
            && (100..=200).contains(&self.improving_margin_percent)
            && self.improving_lmp_bonus <= 32
            && (4..=32).contains(&self.iid_min_depth)
            && (2..=8).contains(&self.iid_reduction)
            && self.iid_reduction < self.iid_min_depth
            && (1..=2).contains(&self.policy_max_depth)
            && (1..=20).contains(&self.policy_tail_percent)
    }
    fn fields(self) -> [(&'static str, i32); 25] {
        [
            ("lmr_v2", i32::from(self.lmr_v2)),
            ("lmr_divisor", i32::from(self.lmr_divisor)),
            ("lmr_cut_bonus", i32::from(self.lmr_cut_bonus)),
            ("improving", i32::from(self.improving)),
            (
                "improving_lmr_discount",
                i32::from(self.improving_lmr_discount),
            ),
            (
                "improving_margin_percent",
                i32::from(self.improving_margin_percent),
            ),
            ("improving_lmp_bonus", i32::from(self.improving_lmp_bonus)),
            ("iid", i32::from(self.iid)),
            ("iid_min_depth", i32::from(self.iid_min_depth)),
            ("iid_reduction", i32::from(self.iid_reduction)),
            ("policy_pruning", i32::from(self.policy_pruning)),
            ("policy_max_depth", i32::from(self.policy_max_depth)),
            ("policy_tail_percent", i32::from(self.policy_tail_percent)),
            ("competitive_tt", i32::from(self.competitive_tt)),
            ("null_move", i32::from(self.null_move)),
            ("null_min_depth", i32::from(self.null_min_depth)),
            ("null_reduction", i32::from(self.null_reduction)),
            ("normalized_scores", i32::from(self.normalized_scores)),
            ("time_stability", i32::from(self.time_stability)),
            ("time_drop", i32::from(self.time_drop)),
            ("growth_min_q8", i32::from(self.growth_min_q8)),
            ("growth_max_q8", i32::from(self.growth_max_q8)),
            ("growth_initial_q8", i32::from(self.growth_initial_q8)),
            ("growth_ema_weight", i32::from(self.growth_ema_weight)),
            ("lmr_v2_policy", i32::from(self.lmr_v2_policy)),
        ]
    }
    fn from_fields(values: [(&str, i32); 25]) -> Result<Self, &'static str> {
        let value = |name: &str| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| *value)
                .ok_or("missing named parameter")
        };
        let flag = |name| match value(name)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("invalid research flag"),
        };
        Ok(Self {
            lmr_v2: flag("lmr_v2")?,
            lmr_v2_policy: flag("lmr_v2_policy")?,
            lmr_divisor: u16::try_from(value("lmr_divisor")?)
                .map_err(|_| "lmr_divisor out of range")?,
            lmr_cut_bonus: u8::try_from(value("lmr_cut_bonus")?)
                .map_err(|_| "lmr_cut_bonus out of range")?,
            improving: flag("improving")?,
            improving_lmr_discount: u8::try_from(value("improving_lmr_discount")?)
                .map_err(|_| "improving_lmr_discount out of range")?,
            improving_margin_percent: u16::try_from(value("improving_margin_percent")?)
                .map_err(|_| "improving_margin_percent out of range")?,
            improving_lmp_bonus: u16::try_from(value("improving_lmp_bonus")?)
                .map_err(|_| "improving_lmp_bonus out of range")?,
            iid: flag("iid")?,
            iid_min_depth: u8::try_from(value("iid_min_depth")?)
                .map_err(|_| "iid_min_depth out of range")?,
            iid_reduction: u8::try_from(value("iid_reduction")?)
                .map_err(|_| "iid_reduction out of range")?,
            policy_pruning: flag("policy_pruning")?,
            policy_max_depth: u8::try_from(value("policy_max_depth")?)
                .map_err(|_| "policy_max_depth out of range")?,
            policy_tail_percent: u8::try_from(value("policy_tail_percent")?)
                .map_err(|_| "policy_tail_percent out of range")?,
            competitive_tt: flag("competitive_tt")?,
            time_stability: u16::try_from(value("time_stability")?)
                .map_err(|_| "time_stability out of range")?,
            time_drop: u16::try_from(value("time_drop")?).map_err(|_| "time_drop out of range")?,
            growth_min_q8: u16::try_from(value("growth_min_q8")?)
                .map_err(|_| "growth_min_q8 out of range")?,
            growth_max_q8: u16::try_from(value("growth_max_q8")?)
                .map_err(|_| "growth_max_q8 out of range")?,
            growth_initial_q8: u16::try_from(value("growth_initial_q8")?)
                .map_err(|_| "growth_initial_q8 out of range")?,
            growth_ema_weight: u16::try_from(value("growth_ema_weight")?)
                .map_err(|_| "growth_ema_weight out of range")?,
            normalized_scores: flag("normalized_scores")?,
            null_move: flag("null_move")?,
            null_min_depth: u8::try_from(value("null_min_depth")?)
                .map_err(|_| "null_min_depth out of range")?,
            null_reduction: u8::try_from(value("null_reduction")?)
                .map_err(|_| "null_reduction out of range")?,
        })
    }
}

#[cfg(test)]
mod named_tests {
    use super::*;
    #[test]
    fn named_profile_preserves_old_versions_and_rejects_ambiguous_fields() {
        let p = SearchProfile::baseline(ScoreContract::Pattern)
            .with_research(ResearchParameters {
                lmr_v2: true,
                improving: true,
                iid: true,
                ..ResearchParameters::OFF
            })
            .unwrap();
        let text = p.to_string();
        assert_eq!(text.parse::<SearchProfile>().unwrap(), p);
        assert!(format!("{text};iid=1").parse::<SearchProfile>().is_err());
        assert!(
            text.replace("iid=1", "iid=2")
                .parse::<SearchProfile>()
                .is_err()
        );
        assert!(
            text.replace("lmr_divisor=4", "lmr_divisor=0")
                .parse::<SearchProfile>()
                .is_err()
        );
        assert!(text.replace(";iid=1", "").parse::<SearchProfile>().is_err());
    }
}

impl ScoreContract {
    /// Pattern/V1 share calibrated teacher-score units. Rational heads expose
    /// their immutable model scale; 10,000 reference units represent one scale.
    pub const fn reference_scale(self) -> i32 {
        match self {
            Self::Pattern | Self::LinearV1 => 10_000,
            Self::RationalV2 { scale } => {
                if scale > 0 {
                    scale
                } else {
                    1
                }
            }
        }
    }
    pub fn reference_score(self, raw: i32) -> i32 {
        (i64::from(raw) * 10_000 / i64::from(self.reference_scale()))
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }
    pub fn raw_threshold(self, reference: i32) -> i32 {
        let raw = i64::from(reference) * i64::from(self.reference_scale()) / 10_000;
        raw.clamp(-10_000_000, 10_000_000) as i32
    }
    pub const fn is_mate_score(score: i32) -> bool {
        score.unsigned_abs() >= crate::score::MATE_THRESHOLD as u32
    }
}

#[cfg(test)]
mod normalization_tests {
    use super::*;
    #[test]
    fn normalized_profile_and_legacy_raw_contracts_are_distinct() {
        let profile = SearchProfile::baseline(ScoreContract::RationalV2 { scale: 500 });
        assert_eq!(profile.raw_threshold(10_000), 500);
        assert_eq!(profile.reference_score(200), 4000);
        assert_eq!(
            profile.to_string().parse::<SearchProfile>().unwrap(),
            profile
        );
        let raw = profile.with_research(ResearchParameters::OFF).unwrap();
        assert!(raw.to_string().starts_with("RMPROFILE1,"));
        assert_eq!(
            raw.to_string()
                .parse::<SearchProfile>()
                .unwrap()
                .raw_threshold(10000),
            10000
        );
        assert_ne!(raw.to_string(), profile.to_string());
        assert!(ScoreContract::is_mate_score(i32::MIN));
    }
}
