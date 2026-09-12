/// Deterministic proof limits; zero in either field disables the solver.
/// Plies include the final winning move. Nodes include cache/certificate visits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofLimits {
    pub max_plies: u8,
    pub max_nodes: u64,
}

/// Independent research ablations of existing V0.10 heuristics. These flags
/// never relax proof, interruption, or transposition-bound validity rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectivityConfig {
    pub reverse_futility: bool,
    pub futility: bool,
    pub razoring: bool,
    pub lmp: bool,
    pub lmr: bool,
    pub iir: bool,
    pub threat_extension: bool,
}

impl SelectivityConfig {
    pub const BASELINE: Self = Self {
        reverse_futility: true,
        futility: true,
        razoring: true,
        lmp: true,
        lmr: true,
        iir: true,
        threat_extension: true,
    };
    pub const OFF: Self = Self {
        reverse_futility: false,
        futility: false,
        razoring: false,
        lmp: false,
        lmr: false,
        iir: false,
        threat_extension: false,
    };
}

impl Default for SelectivityConfig {
    fn default() -> Self {
        Self::BASELINE
    }
}

impl ProofLimits {
    #[must_use]
    pub const fn new(max_plies: u8, max_nodes: u64) -> Self {
        Self {
            max_plies,
            max_nodes,
        }
    }

    #[must_use]
    pub const fn enabled(self) -> bool {
        self.max_plies != 0 && self.max_nodes != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TacticalConfig {
    pub vcf: ProofLimits,
    pub vct: ProofLimits,
    /// Upper memory request, rounded down to a power-of-two bucket count.
    /// The 16 MiB default currently allocates 12 MiB of 48-byte entries.
    pub vct_table_memory_mib: usize,
}

impl Default for TacticalConfig {
    fn default() -> Self {
        EngineConfig::new(EngineConfig::DEFAULT_TT_MEMORY_MIB).tactical()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    tt_memory_mib: usize,
    threads: usize,
    tactical: TacticalConfig,
    interior_vcf: ProofLimits,
    interior_vcf_total_work: u64,
    interior_vct: ProofLimits,
    interior_vct_total_work: u64,
    selectivity: SelectivityConfig,
    root_resistance: bool,
    adaptive_root_candidates: bool,
    profile: Option<crate::SearchProfile>,
    probcut: Option<crate::ProbCutCalibration>,
}

impl EngineConfig {
    pub const DEFAULT_TT_MEMORY_MIB: usize = 64;
    pub const DEFAULT_VCF_MAX_PLIES: u8 = 11;
    pub const DEFAULT_VCF_MAX_NODES: u64 = 2_000;

    #[must_use]
    pub const fn new(tt_memory_mib: usize) -> Self {
        Self {
            tt_memory_mib,
            threads: 1,
            interior_vcf: ProofLimits::new(0, 0),
            interior_vcf_total_work: 0,
            interior_vct: ProofLimits::new(0, 0),
            interior_vct_total_work: 0,
            selectivity: SelectivityConfig::BASELINE,
            root_resistance: true,
            adaptive_root_candidates: false,
            profile: None,
            probcut: None,
            tactical: TacticalConfig {
                vcf: ProofLimits::new(Self::DEFAULT_VCF_MAX_PLIES, Self::DEFAULT_VCF_MAX_NODES),
                vct: ProofLimits::new(9, 4_000),
                vct_table_memory_mib: 16,
            },
        }
    }

    #[must_use]
    pub const fn tt_memory_mib(self) -> usize {
        self.tt_memory_mib
    }

    /// Number of CPU Alpha-Beta workers used by one public search.
    #[must_use]
    pub const fn threads(self) -> usize {
        self.threads
    }

    /// Experimental single-worker ordering probes. `max_nodes` is a total
    /// visit cap per probe, including certificate replay. Zero disables.
    #[must_use]
    pub const fn with_interior_vcf(mut self, limits: ProofLimits, total_work: u64) -> Self {
        self.interior_vcf = limits;
        self.interior_vcf_total_work = total_work;
        self
    }

    /// Configured probe limits and per-search allowance, for experiment identity.
    #[must_use]
    pub const fn interior_vcf(self) -> (ProofLimits, u64) {
        (self.interior_vcf, self.interior_vcf_total_work)
    }

    #[must_use]
    pub const fn with_interior_vct(mut self, limits: ProofLimits, total_work: u64) -> Self {
        self.interior_vct = limits;
        self.interior_vct_total_work = total_work;
        self
    }
    #[must_use]
    pub const fn interior_vct(self) -> (ProofLimits, u64) {
        (self.interior_vct, self.interior_vct_total_work)
    }

    #[must_use]
    pub const fn with_selectivity(mut self, config: SelectivityConfig) -> Self {
        self.selectivity = config;
        self
    }

    #[must_use]
    pub const fn selectivity(self) -> SelectivityConfig {
        self.selectivity
    }

    /// Practical root preference only among verified equal mate-domain losses.
    /// Disabling restores canonical index ties; primary scores never change.
    #[must_use]
    pub const fn with_root_resistance(mut self, enabled: bool) -> Self {
        self.root_resistance = enabled;
        self
    }

    #[must_use]
    pub const fn root_resistance(self) -> bool {
        self.root_resistance
    }

    /// Experimental broader root universe. No sibling is removed; new tactical
    /// and policy candidates can change practical strength, so default is off.
    #[must_use]
    pub const fn with_adaptive_root_candidates(mut self, enabled: bool) -> Self {
        self.adaptive_root_candidates = enabled;
        self
    }

    #[must_use]
    pub const fn adaptive_root_candidates(self) -> bool {
        self.adaptive_root_candidates
    }

    #[must_use]
    pub const fn with_search_profile(mut self, profile: crate::SearchProfile) -> Self {
        self.profile = Some(profile);
        self
    }

    /// Mismatched contracts fail closed to the compatible historical profile.
    #[must_use]
    pub fn effective_profile(self, contract: crate::ScoreContract) -> crate::SearchProfile {
        self.profile
            .filter(|p| p.contract() == contract)
            .unwrap_or(crate::SearchProfile::baseline(contract))
    }

    #[must_use]
    pub const fn search_profile(self) -> Option<crate::SearchProfile> {
        self.profile
    }

    #[must_use]
    pub const fn with_probcut(mut self, calibration: crate::ProbCutCalibration) -> Self {
        self.probcut = Some(calibration);
        self
    }
    #[must_use]
    pub const fn probcut(self) -> Option<crate::ProbCutCalibration> {
        self.probcut
    }

    /// Sets the number of CPU Alpha-Beta workers. Zero is normalized to the
    /// smallest valid team so an invalid configuration cannot reach search.
    #[must_use]
    pub const fn with_threads(mut self, threads: usize) -> Self {
        self.threads = if threads == 0 { 1 } else { threads };
        self
    }

    /// Replaces the ordinary TT capacity while preserving all other settings.
    #[must_use]
    pub const fn with_tt_memory_mib(mut self, memory_mib: usize) -> Self {
        self.tt_memory_mib = memory_mib;
        self
    }

    #[must_use]
    pub const fn tactical(self) -> TacticalConfig {
        self.tactical
    }

    #[must_use]
    pub const fn with_tactical(mut self, tactical: TacticalConfig) -> Self {
        self.tactical = tactical;
        self
    }

    /// Convenient compatibility setter for the cheaper continuous-four solver.
    #[must_use]
    pub const fn with_vcf_limits(mut self, max_plies: u8, max_nodes: u64) -> Self {
        self.tactical.vcf = ProofLimits::new(max_plies, max_nodes);
        self
    }

    #[must_use]
    pub const fn with_vct_limits(mut self, max_plies: u8, max_nodes: u64) -> Self {
        self.tactical.vct = ProofLimits::new(max_plies, max_nodes);
        self
    }

    #[must_use]
    pub const fn with_vct_table_memory(mut self, memory_mib: usize) -> Self {
        self.tactical.vct_table_memory_mib = memory_mib;
        self
    }

    #[must_use]
    pub const fn vcf_max_plies(self) -> u8 {
        self.tactical.vcf.max_plies
    }

    #[must_use]
    pub const fn vcf_max_nodes(self) -> u64 {
        self.tactical.vcf.max_nodes
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self::new(Self::DEFAULT_TT_MEMORY_MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::EngineConfig;

    #[test]
    fn thread_configuration_defaults_to_one_and_normalizes_zero() {
        assert_eq!(EngineConfig::default().threads(), 1);
        assert_eq!(EngineConfig::new(0).with_threads(0).threads(), 1);
        assert_eq!(EngineConfig::new(0).with_threads(8).threads(), 8);
    }
}
