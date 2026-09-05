//! Threat witnesses and the defender obligation. Profiles only gate attacks;
//! responses come from simulated five-window dependencies and actual tactics.
use rustmoku_core::{Move, Stone};

use crate::{
    bitboard::BitBoard256,
    board_state::BoardState,
    line_geometry::LINE_CELLS,
    pattern::{LineKey, ThreatProfile, stone_index},
    tactical::forcing_moves,
};

#[repr(align(64))]
struct MetadataTable([u8; 8 * 65_536]);
static METADATA: MetadataTable = MetadataTable(*include_bytes!(concat!(
    env!("OUT_DIR"),
    "/threat_meta.bin"
)));

fn metadata(key: LineKey, attacker: Stone) -> [u8; 4] {
    let index = usize::from(key.0) * 8 + stone_index(attacker) * 4;
    [
        METADATA.0[index],
        METADATA.0[index + 1],
        METADATA.0[index + 2],
        METADATA.0[index + 3],
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ThreatDescriptor {
    pub(super) gain: Move,
    // The seven forcing variants of ThreatProfile are the concrete vocabulary;
    // construction rejects Quiet/Three. No second synonymous enum is needed.
    pub(super) kind: ThreatProfile,
    pub(super) continuations: BitBoard256,
    pub(super) defenses: BitBoard256,
    pub(super) dependencies: BitBoard256,
}

impl ThreatDescriptor {
    /// Called before the gain is played. The keys also remain available at an
    /// occupied center, so the same descriptor can be validated after the gain.
    pub(super) fn new(board: &BoardState, gain: Move, attacker: Stone) -> Option<Self> {
        if !board.position().is_legal(gain) {
            return None;
        }
        let kind = board.patterns().profile(gain, attacker);
        if kind < ThreatProfile::OpenThree {
            return None;
        }
        let mut descriptor = Self {
            gain,
            kind,
            continuations: BitBoard256::EMPTY,
            defenses: BitBoard256::EMPTY,
            dependencies: BitBoard256::EMPTY,
        };
        descriptor.dependencies.set(gain);
        for (direction, key) in board.patterns().line_keys(gain).into_iter().enumerate() {
            let [class, continuation, cost, dependency] = metadata(key, attacker);
            if class < 2 {
                continue;
            }
            let cells = &LINE_CELLS[gain.index()][direction];
            for (field, &cell) in cells.iter().enumerate() {
                if let Some(cell) = cell {
                    if continuation & (1 << field) != 0 {
                        descriptor.continuations.set(cell);
                    }
                    if cost & (1 << field) != 0 {
                        descriptor.defenses.set(cell);
                    }
                    if dependency & (1 << field) != 0 {
                        descriptor.dependencies.set(cell);
                    }
                }
            }
        }
        Some(descriptor)
    }

    /// Deterministic context verification, separate from the full position key.
    /// All descriptor fields participate, including occupied dependency cells.
    pub(super) fn signature(self) -> u64 {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for value in [self.gain.index() as u64, self.kind as u64] {
            signature = (signature ^ value).wrapping_mul(0x100_0000_01b3);
        }
        for bits in [self.continuations, self.defenses, self.dependencies] {
            // Fixed four-word mixing; no set-bit/board scan to form cache keys.
            for word in bits.words() {
                signature = (signature ^ word).wrapping_mul(0x100_0000_01b3);
            }
        }
        signature
    }

    pub(super) fn responses(self, board: &BoardState, attacker: Stone) -> BitBoard256 {
        let empty = board.patterns().empty_cells();
        let mut responses = self
            .defenses
            .union(forcing_moves(board.patterns(), attacker.opponent()))
            .intersection(empty);
        // Called only after immediate facts: an untouched OpenThree has no
        // winning point yet and can win in exactly three attacker-turn plies.
        // Outside the witness/counter-threat set every reply has that distance.
        // One canonical representative preserves the slowest-defense PV tie.
        if let Some(at) = empty.and_not(responses).iter().next() {
            responses.set(at);
        }
        responses
    }
}

pub(crate) fn attacks(board: &crate::PatternState, attacker: Stone) -> BitBoard256 {
    board.moves_at_least(attacker, ThreatProfile::OpenThree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_classifier;

    #[test]
    fn exhaustive_tactical_metadata_matches_simulation() {
        for key in 0..=u16::MAX {
            for attacker in [Stone::Black, Stone::White] {
                assert_eq!(
                    metadata(LineKey(key), attacker),
                    line_classifier::tactical_metadata(key, 1 + stone_index(attacker) as u8)
                );
            }
        }
        assert_eq!(std::mem::size_of::<MetadataTable>(), 524_288);
    }
}
