#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    tt_memory_mib: usize,
}

impl EngineConfig {
    pub const DEFAULT_TT_MEMORY_MIB: usize = 64;

    #[must_use]
    pub const fn new(tt_memory_mib: usize) -> Self {
        Self { tt_memory_mib }
    }

    #[must_use]
    pub const fn tt_memory_mib(self) -> usize {
        self.tt_memory_mib
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self::new(Self::DEFAULT_TT_MEMORY_MIB)
    }
}
