/// Deterministic proof limits; zero in either field disables the solver.
/// Plies include the final winning move. Nodes include cache/certificate visits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofLimits {
    pub max_plies: u8,
    pub max_nodes: u64,
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
