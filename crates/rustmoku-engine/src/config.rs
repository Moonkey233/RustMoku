#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    tt_memory_mib: usize,
    vcf_max_plies: u8,
    vcf_max_nodes: u64,
}

impl EngineConfig {
    pub const DEFAULT_TT_MEMORY_MIB: usize = 64;
    pub const DEFAULT_VCF_MAX_PLIES: u8 = 11;
    pub const DEFAULT_VCF_MAX_NODES: u64 = 2_000;

    #[must_use]
    pub const fn new(tt_memory_mib: usize) -> Self {
        Self {
            tt_memory_mib,
            vcf_max_plies: Self::DEFAULT_VCF_MAX_PLIES,
            vcf_max_nodes: Self::DEFAULT_VCF_MAX_NODES,
        }
    }

    #[must_use]
    pub const fn tt_memory_mib(self) -> usize {
        self.tt_memory_mib
    }

    /// Deterministic per-public-search VCF limits. Zero in either field disables
    /// the root attempt. Proof depth counts actual plies through the final win.
    #[must_use]
    pub const fn with_vcf_limits(mut self, max_plies: u8, max_nodes: u64) -> Self {
        self.vcf_max_plies = max_plies;
        self.vcf_max_nodes = max_nodes;
        self
    }

    #[must_use]
    pub const fn vcf_max_plies(self) -> u8 {
        self.vcf_max_plies
    }

    #[must_use]
    pub const fn vcf_max_nodes(self) -> u64 {
        self.vcf_max_nodes
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self::new(Self::DEFAULT_TT_MEMORY_MIB)
    }
}
