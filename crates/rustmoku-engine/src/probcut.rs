//! Statistical experimental cutoffs have no TT bound authority. Calibration is
//! bound to model bytes and the complete scoring/selectivity profile.
use crate::{ScoreContract, SearchProfile, SelectivityConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbCutBucket {
    pub deep: u8,
    pub shallow: u8,
    pub phase: u8,
    pub slope_q16: i32,
    pub intercept: i32,
    pub tail: i32,
    pub min_shallow: i32,
    pub max_shallow: i32,
    pub training_samples: u32,
    pub heldout_samples: u32,
    pub heldout_false_cuts: u32,
}

impl ProbCutBucket {
    fn valid(self) -> bool {
        (3..=32).contains(&self.deep)
            && self.shallow >= 1
            && self.shallow <= self.deep - 2
            && self.phase <= 2
            && (1..=4 * 65536).contains(&self.slope_q16)
            && self.intercept.unsigned_abs() <= 10_000_000
            && (0..=10_000_000).contains(&self.tail)
            && self.min_shallow >= -10_000_000
            && self.max_shallow <= 10_000_000
            && self.min_shallow < self.max_shallow
            && self.training_samples >= 64
            && self.heldout_samples >= 32
            && self.heldout_false_cuts == 0
    }
    pub(crate) fn lower_prediction(self, shallow_score: i32) -> i64 {
        i64::from(shallow_score) * i64::from(self.slope_q16) / 65536 + i64::from(self.intercept)
            - i64::from(self.tail)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbCutCalibration {
    model: [u8; 32],
    profile: SearchProfile,
    selectivity: SelectivityConfig,
    buckets: [Option<ProbCutBucket>; 16],
}

impl ProbCutCalibration {
    pub fn new(
        model: [u8; 32],
        profile: SearchProfile,
        selectivity: SelectivityConfig,
        buckets: &[ProbCutBucket],
    ) -> Result<Self, &'static str> {
        if buckets.is_empty() || buckets.len() > 16 || buckets.iter().any(|b| !b.valid()) {
            return Err(
                "ProbCut requires 1..16 validated buckets with >=64 training and >=32 heldout samples and zero heldout false cuts",
            );
        }
        let mut result = Self {
            model,
            profile,
            selectivity,
            buckets: [None; 16],
        };
        for (i, bucket) in buckets.iter().enumerate() {
            if buckets[..i]
                .iter()
                .any(|old| old.deep == bucket.deep && old.phase == bucket.phase)
            {
                return Err("duplicate ProbCut depth/phase bucket");
            }
            result.buckets[i] = Some(*bucket);
        }
        Ok(result)
    }
    #[must_use]
    pub fn matches(
        self,
        model: Option<[u8; 32]>,
        profile: SearchProfile,
        selectivity: SelectivityConfig,
    ) -> bool {
        model == Some(self.model) && profile == self.profile && selectivity == self.selectivity
    }
    pub(crate) fn bucket(self, depth: u8, stones: usize) -> Option<ProbCutBucket> {
        let phase = if stones < 20 {
            0
        } else if stones < 80 {
            1
        } else {
            2
        };
        self.buckets
            .into_iter()
            .flatten()
            .find(|b| b.deep == depth && b.phase == phase)
    }
    #[must_use]
    pub const fn contract(self) -> ScoreContract {
        self.profile.contract()
    }
}

impl std::str::FromStr for ProbCutCalibration {
    type Err = &'static str;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() > 8192 {
            return Err("ProbCut calibration exceeds 8192 bytes");
        }
        let mut lines = text.lines();
        if lines.next() != Some("RMPROBCUT1") {
            return Err("unsupported ProbCut calibration version");
        }
        let hex = lines.next().ok_or("missing model fingerprint")?;
        if hex.len() != 64 || !hex.is_ascii() {
            return Err("invalid model fingerprint");
        }
        let mut model = [0; 32];
        for (index, byte) in model.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
                .map_err(|_| "invalid model fingerprint")?;
        }
        let profile = lines.next().ok_or("missing profile")?.parse()?;
        let flags = lines.next().ok_or("missing selectivity flags")?.as_bytes();
        if flags.len() != 7 || flags.iter().any(|byte| !matches!(byte, b'0' | b'1')) {
            return Err("invalid selectivity flags");
        }
        let selection = SelectivityConfig {
            reverse_futility: flags[0] == b'1',
            futility: flags[1] == b'1',
            razoring: flags[2] == b'1',
            lmp: flags[3] == b'1',
            lmr: flags[4] == b'1',
            iir: flags[5] == b'1',
            threat_extension: flags[6] == b'1',
        };
        let mut buckets = Vec::new();
        for line in lines {
            if buckets.len() == 16 {
                return Err("too many ProbCut buckets");
            }
            let values: Vec<i64> = line
                .split(',')
                .map(|value| value.parse().map_err(|_| "invalid bucket integer"))
                .collect::<Result<_, _>>()?;
            if values.len() != 11 {
                return Err("invalid bucket field count");
            }
            let byte = |index: usize| {
                u8::try_from(values[index]).map_err(|_| "bucket integer out of range")
            };
            let signed = |index: usize| {
                i32::try_from(values[index]).map_err(|_| "bucket integer out of range")
            };
            let unsigned = |index: usize| {
                u32::try_from(values[index]).map_err(|_| "bucket integer out of range")
            };
            buckets.push(ProbCutBucket {
                deep: byte(0)?,
                shallow: byte(1)?,
                phase: byte(2)?,
                slope_q16: signed(3)?,
                intercept: signed(4)?,
                tail: signed(5)?,
                training_samples: unsigned(6)?,
                heldout_samples: unsigned(7)?,
                heldout_false_cuts: unsigned(8)?,
                min_shallow: signed(9)?,
                max_shallow: signed(10)?,
            });
        }
        Self::new(model, profile, selection, &buckets)
    }
}

impl std::fmt::Display for ProbCutCalibration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "RMPROBCUT1")?;
        for byte in self.model {
            write!(f, "{byte:02x}")?;
        }
        writeln!(f, "\n{}", self.profile)?;
        let s = self.selectivity;
        for flag in [
            s.reverse_futility,
            s.futility,
            s.razoring,
            s.lmp,
            s.lmr,
            s.iir,
            s.threat_extension,
        ] {
            write!(f, "{}", u8::from(flag))?;
        }
        writeln!(f)?;
        for b in self.buckets.into_iter().flatten() {
            writeln!(
                f,
                "{},{},{},{},{},{},{},{},{},{},{}",
                b.deep,
                b.shallow,
                b.phase,
                b.slope_q16,
                b.intercept,
                b.tail,
                b.training_samples,
                b.heldout_samples,
                b.heldout_false_cuts,
                b.min_shallow,
                b.max_shallow
            )?;
        }
        Ok(())
    }
}
