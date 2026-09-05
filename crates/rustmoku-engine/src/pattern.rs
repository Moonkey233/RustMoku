use rustmoku_core::Stone;

/// Offsets [-4,-3,-2,-1,+1,+2,+3,+4] occupy successive low-to-high
/// two-bit fields. Empty=00, Black=01, White=10, Wall=11; center is omitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LineKey(pub(crate) u16);

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum DirectionalThreat {
    #[default]
    Quiet,
    Three,
    OpenThree,
    Four,
    OpenFour,
    Five,
}

#[cfg(test)]
impl DirectionalThreat {
    const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Quiet,
            1 => Self::Three,
            2 => Self::OpenThree,
            3 => Self::Four,
            4 => Self::OpenFour,
            5 => Self::Five,
            _ => panic!("build-generated directional code must be in 0..6"),
        }
    }
}

/// Two packed bytes per table entry, black first, then white.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PatternPair([u8; 2]);

#[repr(align(64))]
struct PatternTable([u8; 2 * 65_536]);
static PATTERN_TABLE: PatternTable =
    PatternTable(*include_bytes!(concat!(env!("OUT_DIR"), "/patterns.bin")));

impl PatternPair {
    #[inline]
    pub(crate) fn lookup(key: LineKey) -> Self {
        let index = usize::from(key.0) * 2;
        Self([PATTERN_TABLE.0[index], PATTERN_TABLE.0[index + 1]])
    }

    #[cfg(test)]
    pub(crate) fn for_stone(self, stone: Stone) -> DirectionalThreat {
        DirectionalThreat::from_code(self.0[stone_index(stone)])
    }
}

/// Two 12-bit keys packed into a u32 (Black bits 0..12, White 16..28).
/// Each key contains four 3-bit directional classes; the unused bits stay zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DirectionSet(u32);

impl DirectionSet {
    pub(crate) fn new(pairs: [PatternPair; 4]) -> Self {
        let mut result = Self(0);
        for (direction, pair) in pairs.into_iter().enumerate() {
            result.replace(direction as u8, pair);
        }
        result
    }

    /// Returns whether the directional classes changed, even if the line did.
    #[inline]
    pub(crate) fn replace(&mut self, direction: u8, pair: PatternPair) -> bool {
        let shift = direction * 3;
        let value = u32::from(pair.0[0]) | (u32::from(pair.0[1]) << 16);
        let updated = (self.0 & !(0x0007_0007 << shift)) | (value << shift);
        let changed = self.0 != updated;
        self.0 = updated;
        changed
    }

    #[inline]
    pub(crate) fn profiles(self) -> [ThreatProfile; 2] {
        [
            PROFILE_TABLE[(self.0 & 0xfff) as usize],
            PROFILE_TABLE[((self.0 >> 16) & 0xfff) as usize],
        ]
    }
}

pub(crate) const fn stone_index(stone: Stone) -> usize {
    match stone {
        Stone::Black => 0,
        Stone::White => 1,
    }
}

/// Ordered structural classes, independent of evaluator numeric weights.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub(crate) enum ThreatProfile {
    #[default]
    Quiet,
    Three,
    OpenThree,
    DoubleThree,
    Four,
    FourThree,
    DoubleFour,
    OpenFour,
    WinningMove,
}

impl ThreatProfile {
    pub(crate) const COUNT: usize = 9;

    #[cfg(test)]
    pub(crate) fn from_directions(directions: [DirectionalThreat; 4]) -> Self {
        let mut fours = 0;
        let mut open_threes = 0;
        let mut threes = 0;
        let mut open_four = false;
        for direction in directions {
            match direction {
                DirectionalThreat::Five => return Self::WinningMove,
                DirectionalThreat::OpenFour => open_four = true,
                DirectionalThreat::Four => fours += 1,
                DirectionalThreat::OpenThree => open_threes += 1,
                DirectionalThreat::Three => threes += 1,
                DirectionalThreat::Quiet => {}
            }
        }
        if open_four {
            Self::OpenFour
        } else if fours >= 2 {
            Self::DoubleFour
        } else if fours > 0 && open_threes > 0 {
            Self::FourThree
        } else if fours > 0 {
            Self::Four
        } else if open_threes >= 2 {
            Self::DoubleThree
        } else if open_threes > 0 {
            Self::OpenThree
        } else if threes > 0 {
            Self::Three
        } else {
            Self::Quiet
        }
    }
}

// Measured profile aggregation hotspot: 4096 one-byte entries replace branches
// per color/center. Codes 6/7 are unreachable from the directional table.
static PROFILE_TABLE: [ThreatProfile; 4096] = profile_table();

const fn profile_table() -> [ThreatProfile; 4096] {
    let mut table = [ThreatProfile::Quiet; 4096];
    let mut key = 0;
    while key < table.len() {
        let mut counts = [0; 6];
        let mut direction = 0;
        while direction < 4 {
            let code = (key >> (direction * 3)) & 7;
            if code < 6 {
                counts[code] += 1;
            }
            direction += 1;
        }
        table[key] = if counts[5] > 0 {
            ThreatProfile::WinningMove
        } else if counts[4] > 0 {
            ThreatProfile::OpenFour
        } else if counts[3] >= 2 {
            ThreatProfile::DoubleFour
        } else if counts[3] > 0 && counts[2] > 0 {
            ThreatProfile::FourThree
        } else if counts[3] > 0 {
            ThreatProfile::Four
        } else if counts[2] >= 2 {
            ThreatProfile::DoubleThree
        } else if counts[2] > 0 {
            ThreatProfile::OpenThree
        } else if counts[1] > 0 {
            ThreatProfile::Three
        } else {
            ThreatProfile::Quiet
        };
        key += 1;
    }
    table
}

#[cfg(test)]
#[path = "line_classifier.rs"]
mod line_classifier;

#[cfg(test)]
mod tests {
    use super::*;
    use DirectionalThreat::{Five, Four, OpenFour, OpenThree, Quiet, Three};

    fn key(line: &str) -> LineKey {
        assert_eq!(line.len(), 9);
        let mut key = 0;
        let mut field = 0;
        for (index, cell) in line.bytes().enumerate() {
            if index == 4 {
                assert_eq!(cell, b'?');
                continue;
            }
            let code = match cell {
                b'.' => 0,
                b'X' => 1,
                b'O' => 2,
                b'#' => 3,
                _ => panic!("test cell"),
            };
            key |= code << (2 * field);
            field += 1;
        }
        LineKey(key)
    }

    #[test]
    fn directional_semantics_cover_contiguous_broken_and_wall_shapes() {
        for (line, expected) in [
            ("XXXX?....", Five),
            (".XXX?....", OpenFour),
            ("#XXX?....", Four),
            (".XX.?X...", Four),
            (".X.X?X...", Four),
            ("..XX?....", OpenThree),
            ("..X.?X...", OpenThree),
            ("..X.?.X..", Three),
            ("#XXX?O...", Quiet),
            ("####?XX..", Three),
            ("####?....", Quiet),
            ("....?....", Quiet),
        ] {
            assert_eq!(
                PatternPair::lookup(key(line)).for_stone(Stone::Black),
                expected,
                "{line}"
            );
        }
    }

    #[test]
    fn table_matches_all_semantic_keys_and_color_and_reflection_symmetry() {
        for key in 0..=u16::MAX {
            let pair = PatternPair::lookup(LineKey(key));
            let mut swapped = 0;
            let mut reversed = 0;
            for field in 0..8 {
                let cell = (key >> (2 * field)) & 3;
                let other = match cell {
                    1 => 2,
                    2 => 1,
                    _ => cell,
                };
                swapped |= other << (2 * field);
                reversed |= cell << (2 * (7 - field));
            }
            for stone in [Stone::Black, Stone::White] {
                let color = 1 + stone_index(stone) as u8;
                assert_eq!(
                    pair.for_stone(stone) as u8,
                    line_classifier::classify(key, color)
                );
                assert_eq!(
                    pair.for_stone(stone),
                    PatternPair::lookup(LineKey(swapped)).for_stone(stone.opponent())
                );
                assert_eq!(
                    pair.for_stone(stone),
                    PatternPair::lookup(LineKey(reversed)).for_stone(stone)
                );
            }
        }
        assert_eq!(std::mem::size_of::<PatternPair>(), 2);
        assert_eq!(std::mem::size_of::<PatternTable>(), 131_072);
        assert_eq!(std::mem::size_of::<ThreatProfile>(), 1);
    }

    #[test]
    fn profiles_recognize_structural_combinations() {
        for (directions, expected) in [
            ([Five, Quiet, Quiet, Quiet], ThreatProfile::WinningMove),
            ([OpenFour, Quiet, Quiet, Quiet], ThreatProfile::OpenFour),
            ([Four, Four, Quiet, Quiet], ThreatProfile::DoubleFour),
            ([Four, OpenThree, Quiet, Quiet], ThreatProfile::FourThree),
            ([Four, Three, Quiet, Quiet], ThreatProfile::Four),
            (
                [OpenThree, OpenThree, Quiet, Quiet],
                ThreatProfile::DoubleThree,
            ),
            ([OpenThree, Quiet, Quiet, Quiet], ThreatProfile::OpenThree),
            ([Three, Quiet, Quiet, Quiet], ThreatProfile::Three),
            ([Quiet; 4], ThreatProfile::Quiet),
        ] {
            assert_eq!(ThreatProfile::from_directions(directions), expected);
        }
    }

    #[test]
    fn all_structural_combinations_match_branch_reference() {
        for a in 0..6 {
            for b in 0..6 {
                for c in 0..6 {
                    for d in 0..6 {
                        let codes = [a, b, c, d];
                        let directions = codes.map(DirectionalThreat::from_code);
                        let pairs = codes.map(|code| PatternPair([code, code]));
                        assert_eq!(
                            DirectionSet::new(pairs).profiles(),
                            [ThreatProfile::from_directions(directions); 2]
                        );
                    }
                }
            }
        }
    }
}
