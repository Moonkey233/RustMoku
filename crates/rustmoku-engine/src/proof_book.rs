//! Stable, explicitly encoded Freestyle winning strategies.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use rustmoku_core::{
    CELL_COUNT, CanonicalPosition, CanonicalPositionKey, Game, Move, Position, RuleSet, Stone,
};

const MAGIC: &[u8; 8] = b"RMPBOOK1";
const VERSION: u16 = 1;
const MAX_ROOTS: usize = 1_024;
const MAX_ENTRIES: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProofSource {
    Vcf,
    Vct,
    ProofBook,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProofDistance {
    /// Exact under the proof source's declared semantics (for example VCF's
    /// forcing vocabulary, or an immediate board fact).
    Exact(u8),
    /// The strategy wins within this many plies, but may not be shortest.
    AtMost(u8),
}

impl ProofDistance {
    #[must_use]
    pub const fn plies(self) -> u8 {
        match self {
            Self::Exact(plies) | Self::AtMost(plies) => plies,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proof {
    pub source: ProofSource,
    /// `AtMost(n)` makes a book-derived `MATE_SCORE - n` a proven lower
    /// bound, not a claim that the globally shortest win is exactly `n`.
    pub distance: ProofDistance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct EntryKey {
    pub(crate) attacker: StoneKey,
    pub(crate) position: CanonicalPositionKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StoneKey {
    Black,
    White,
}

impl From<Stone> for StoneKey {
    fn from(value: Stone) -> Self {
        match value {
            Stone::Black => Self::Black,
            Stone::White => Self::White,
        }
    }
}

impl From<StoneKey> for Stone {
    fn from(value: StoneKey) -> Self {
        match value {
            StoneKey::Black => Self::Black,
            StoneKey::White => Self::White,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoredAction {
    AttackerMove(Move),
    DefenderAll,
    Immediate,
    Vcf {
        best_move: Option<Move>,
        max_plies: u8,
        max_nodes: u64,
    },
    Vct {
        best_move: Option<Move>,
        max_plies: u8,
        max_nodes: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StoredEntry {
    pub(crate) key: EntryKey,
    pub(crate) distance: ProofDistance,
    pub(crate) action: StoredAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredRoot {
    pub(crate) attacker: Stone,
    pub(crate) moves: Vec<Move>,
    pub(crate) key: CanonicalPositionKey,
}

/// Parsed and structurally valid bytes. This type is deliberately not accepted
/// by the runtime engine until independent strategy verification succeeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofBook {
    pub(crate) roots: Vec<StoredRoot>,
    pub(crate) entries: Vec<StoredEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofBookMetadata {
    pub version: u16,
    pub rules: RuleSet,
    pub roots: usize,
    pub entries: usize,
    pub black_roots: usize,
    pub white_roots: usize,
    pub sources: ProofBookSourceSummary,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProofBookSourceSummary {
    pub attacker_moves: usize,
    pub defender_nodes: usize,
    pub immediate_leaves: usize,
    pub vcf_leaves: usize,
    pub vct_leaves: usize,
}

impl ProofBook {
    pub(crate) fn new(mut roots: Vec<StoredRoot>, mut entries: Vec<StoredEntry>) -> Self {
        roots.sort_by(|left, right| {
            (
                StoneKey::from(left.attacker),
                left.key,
                left.moves.as_slice(),
            )
                .cmp(&(
                    StoneKey::from(right.attacker),
                    right.key,
                    right.moves.as_slice(),
                ))
        });
        entries.sort_by_key(|entry| entry.key);
        Self { roots, entries }
    }

    #[must_use]
    pub fn metadata(&self) -> ProofBookMetadata {
        metadata(&self.roots, &self.entries)
    }

    pub fn read_from_path(path: impl AsRef<Path>) -> Result<Self, ProofBookError> {
        let mut file = File::open(path).map_err(ProofBookError::Io)?;
        Self::read_from(&mut file)
    }

    pub fn read_from(reader: &mut impl Read) -> Result<Self, ProofBookError> {
        let mut decoder = Decoder { reader };
        let mut magic = [0_u8; 8];
        decoder.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(ProofBookError::Invalid("invalid Proof Book magic"));
        }
        if decoder.u16()? != VERSION {
            return Err(ProofBookError::Invalid("unsupported Proof Book version"));
        }
        if decoder.u8()? != 0 {
            return Err(ProofBookError::Invalid("unsupported Proof Book rules"));
        }
        let root_count = checked_count(decoder.u32()?, MAX_ROOTS, "root count")?;
        let entry_count = checked_count(decoder.u32()?, MAX_ENTRIES, "entry count")?;
        let mut roots = Vec::with_capacity(root_count);
        for _ in 0..root_count {
            let attacker = decode_stone(decoder.u8()?)?;
            let move_count = usize::from(decoder.u16()?);
            if move_count > CELL_COUNT {
                return Err(ProofBookError::Invalid("root move count exceeds the board"));
            }
            let mut moves = Vec::with_capacity(move_count);
            for _ in 0..move_count {
                moves.push(decode_move(decoder.u8()?)?);
            }
            let key = decoder.key()?;
            roots.push(StoredRoot {
                attacker,
                moves,
                key,
            });
        }
        let mut entries = Vec::with_capacity(entry_count);
        let mut previous = None;
        for _ in 0..entry_count {
            let key = EntryKey {
                attacker: StoneKey::from(decode_stone(decoder.u8()?)?),
                position: decoder.key()?,
            };
            if previous.is_some_and(|value| value >= key) {
                return Err(ProofBookError::Invalid(
                    "Proof Book entries are duplicate or not strictly sorted",
                ));
            }
            previous = Some(key);
            let distance = decode_distance(decoder.u8()?, decoder.u8()?)?;
            let action = match decoder.u8()? {
                0 => StoredAction::AttackerMove(decode_move(decoder.u8()?)?),
                1 => StoredAction::DefenderAll,
                2 => StoredAction::Immediate,
                3 => StoredAction::Vcf {
                    best_move: decode_optional_move(&mut decoder)?,
                    max_plies: decoder.u8()?,
                    max_nodes: decoder.u64()?,
                },
                4 => StoredAction::Vct {
                    best_move: decode_optional_move(&mut decoder)?,
                    max_plies: decoder.u8()?,
                    max_nodes: decoder.u64()?,
                },
                _ => return Err(ProofBookError::Invalid("invalid Proof Book action tag")),
            };
            entries.push(StoredEntry {
                key,
                distance,
                action,
            });
        }
        let mut trailing = [0_u8; 1];
        match decoder.reader.read(&mut trailing) {
            Ok(0) => {}
            Ok(_) => return Err(ProofBookError::Invalid("trailing Proof Book bytes")),
            Err(error) => return Err(ProofBookError::Io(error)),
        }
        Ok(Self { roots, entries })
    }

    pub fn write_to_path(&self, path: impl AsRef<Path>) -> Result<(), ProofBookError> {
        atomic_write(path.as_ref(), |file| self.write_to(file)).map_err(ProofBookError::Io)
    }

    pub fn write_to(&self, writer: &mut impl Write) -> Result<(), ProofBookError> {
        writer.write_all(MAGIC).map_err(ProofBookError::Io)?;
        writer
            .write_all(&VERSION.to_le_bytes())
            .map_err(ProofBookError::Io)?;
        writer.write_all(&[0]).map_err(ProofBookError::Io)?;
        write_count(writer, self.roots.len())?;
        write_count(writer, self.entries.len())?;
        for root in &self.roots {
            writer
                .write_all(&[encode_stone(root.attacker)])
                .map_err(ProofBookError::Io)?;
            let count = u16::try_from(root.moves.len())
                .map_err(|_| ProofBookError::Invalid("too many root moves"))?;
            writer
                .write_all(&count.to_le_bytes())
                .map_err(ProofBookError::Io)?;
            for &at in &root.moves {
                writer
                    .write_all(&[move_byte(at)])
                    .map_err(ProofBookError::Io)?;
            }
            writer
                .write_all(root.key.as_bytes())
                .map_err(ProofBookError::Io)?;
        }
        for entry in &self.entries {
            writer
                .write_all(&[encode_stone(entry.key.attacker.into())])
                .map_err(ProofBookError::Io)?;
            writer
                .write_all(entry.key.position.as_bytes())
                .map_err(ProofBookError::Io)?;
            let (distance_tag, plies) = match entry.distance {
                ProofDistance::Exact(plies) => (0, plies),
                ProofDistance::AtMost(plies) => (1, plies),
            };
            writer
                .write_all(&[distance_tag, plies])
                .map_err(ProofBookError::Io)?;
            match entry.action {
                StoredAction::AttackerMove(at) => writer
                    .write_all(&[0, move_byte(at)])
                    .map_err(ProofBookError::Io)?,
                StoredAction::DefenderAll => writer.write_all(&[1]).map_err(ProofBookError::Io)?,
                StoredAction::Immediate => writer.write_all(&[2]).map_err(ProofBookError::Io)?,
                StoredAction::Vcf {
                    best_move,
                    max_plies,
                    max_nodes,
                } => {
                    writer.write_all(&[3]).map_err(ProofBookError::Io)?;
                    encode_optional_move(writer, best_move)?;
                    writer.write_all(&[max_plies]).map_err(ProofBookError::Io)?;
                    writer
                        .write_all(&max_nodes.to_le_bytes())
                        .map_err(ProofBookError::Io)?;
                }
                StoredAction::Vct {
                    best_move,
                    max_plies,
                    max_nodes,
                } => {
                    writer.write_all(&[4]).map_err(ProofBookError::Io)?;
                    encode_optional_move(writer, best_move)?;
                    writer.write_all(&[max_plies]).map_err(ProofBookError::Io)?;
                    writer
                        .write_all(&max_nodes.to_le_bytes())
                        .map_err(ProofBookError::Io)?;
                }
            }
        }
        Ok(())
    }

    pub fn verify(self) -> Result<VerifiedProofBook, ProofBookError> {
        if self.roots.is_empty() {
            return Err(ProofBookError::Invalid("Proof Book has no roots"));
        }
        let entries: BTreeMap<_, _> = self
            .entries
            .iter()
            .map(|entry| (entry.key, *entry))
            .collect();
        if entries.len() != self.entries.len() {
            return Err(ProofBookError::Invalid("duplicate Proof Book entry"));
        }
        let mut visited = BTreeSet::new();
        let mut seen_roots = BTreeSet::new();
        for root in &self.roots {
            let mut game = Game::new(RuleSet::Freestyle);
            for &at in &root.moves {
                game.play_move(at)
                    .map_err(|_| ProofBookError::Invalid("illegal move in Proof Book root"))?;
            }
            let canonical = CanonicalPosition::new(game.position());
            if canonical.key() != root.key {
                return Err(ProofBookError::Invalid("Proof Book root identity mismatch"));
            }
            let root_identity = (StoneKey::from(root.attacker), root.key);
            if !seen_roots.insert(root_identity) {
                return Err(ProofBookError::Invalid("duplicate Proof Book root"));
            }
            let mut stack = BTreeSet::new();
            verify_position(
                game.position(),
                root.attacker,
                &entries,
                &mut visited,
                &mut stack,
            )?;
        }
        if visited.len() != entries.len() {
            return Err(ProofBookError::Invalid("unreachable Proof Book entry"));
        }
        Ok(VerifiedProofBook {
            roots: self.roots,
            entries: self.entries,
        })
    }
}

fn verify_position(
    position: &Position,
    attacker: Stone,
    entries: &BTreeMap<EntryKey, StoredEntry>,
    visited: &mut BTreeSet<EntryKey>,
    stack: &mut BTreeSet<EntryKey>,
) -> Result<ProofDistance, ProofBookError> {
    if let Some(winner) = position.winner() {
        return if winner == attacker {
            Ok(ProofDistance::Exact(0))
        } else {
            Err(ProofBookError::Invalid("strategy reaches an opponent win"))
        };
    }
    if position.is_full() {
        return Err(ProofBookError::Invalid("strategy reaches a draw"));
    }
    let canonical = CanonicalPosition::new(position);
    let key = EntryKey {
        attacker: attacker.into(),
        position: canonical.key(),
    };
    let entry = entries
        .get(&key)
        .ok_or(ProofBookError::Invalid("missing strategy entry"))?;
    if !stack.insert(key) {
        return Err(ProofBookError::Invalid("cycle in Proof Book strategy"));
    }
    visited.insert(key);
    let computed = match entry.action {
        StoredAction::AttackerMove(stored) => {
            if position.side_to_move() != attacker {
                return Err(ProofBookError::Invalid("attacker action on defender turn"));
            }
            let at = canonical.move_to_original(stored);
            if !position.is_legal(at) {
                return Err(ProofBookError::Invalid("illegal transformed attacker move"));
            }
            let mut child = position.clone();
            child
                .make_move(at)
                .map_err(|_| ProofBookError::Invalid("illegal attacker transition"))?;
            add_one(verify_position(&child, attacker, entries, visited, stack)?)
        }
        StoredAction::DefenderAll => {
            if position.side_to_move() == attacker {
                return Err(ProofBookError::Invalid("defender action on attacker turn"));
            }
            let mut longest = 0;
            let mut found = false;
            for at in Move::all().filter(|&at| position.is_legal(at)) {
                found = true;
                let mut child = position.clone();
                child
                    .make_move(at)
                    .map_err(|_| ProofBookError::Invalid("illegal defender transition"))?;
                longest = longest
                    .max(verify_position(&child, attacker, entries, visited, stack)?.plies());
            }
            if !found {
                return Err(ProofBookError::Invalid(
                    "nonterminal defender has no legal move",
                ));
            }
            ProofDistance::AtMost(
                longest
                    .checked_add(1)
                    .ok_or(ProofBookError::Invalid("proof distance overflow"))?,
            )
        }
        StoredAction::Immediate => {
            if position.side_to_move() == attacker {
                return Err(ProofBookError::Invalid(
                    "attacker immediate leaf omits its required action",
                ));
            }
            let plies = crate::offline::verify_immediate(position, attacker)
                .ok_or(ProofBookError::Invalid("invalid immediate proof leaf"))?;
            ProofDistance::Exact(plies)
        }
        StoredAction::Vcf {
            best_move,
            max_plies,
            max_nodes,
        } => {
            let (plies, fresh_move) = crate::offline::verify_tactical_line(
                position, attacker, false, max_plies, max_nodes,
            )
            .ok_or(ProofBookError::Invalid("VCF proof leaf did not verify"))?;
            validate_tactical_move(position, canonical, best_move, fresh_move)?;
            ProofDistance::AtMost(plies)
        }
        StoredAction::Vct {
            best_move,
            max_plies,
            max_nodes,
        } => {
            let (plies, fresh_move) = crate::offline::verify_tactical_line(
                position, attacker, true, max_plies, max_nodes,
            )
            .ok_or(ProofBookError::Invalid("VCT proof leaf did not verify"))?;
            validate_tactical_move(position, canonical, best_move, fresh_move)?;
            ProofDistance::AtMost(plies)
        }
    };
    stack.remove(&key);
    if computed != entry.distance {
        return Err(ProofBookError::Invalid("inconsistent Proof Book distance"));
    }
    Ok(computed)
}

fn add_one(distance: ProofDistance) -> ProofDistance {
    ProofDistance::AtMost(distance.plies().saturating_add(1))
}

/// Immutable, independently verified strategy data accepted by the runtime engine.
#[derive(Debug)]
pub struct VerifiedProofBook {
    roots: Vec<StoredRoot>,
    entries: Vec<StoredEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofBookHit {
    pub best_move: Move,
    pub distance: ProofDistance,
}

impl VerifiedProofBook {
    #[must_use]
    pub fn metadata(&self) -> ProofBookMetadata {
        metadata(&self.roots, &self.entries)
    }

    #[must_use]
    pub fn query(&self, position: &Position) -> Option<ProofBookHit> {
        if position.rules() != RuleSet::Freestyle {
            return None;
        }
        let canonical = CanonicalPosition::new(position);
        let key = EntryKey {
            attacker: position.side_to_move().into(),
            position: canonical.key(),
        };
        let index = self
            .entries
            .binary_search_by_key(&key, |entry| entry.key)
            .ok()?;
        let entry = self.entries[index];
        let stored = match entry.action {
            StoredAction::AttackerMove(stored) => stored,
            StoredAction::Vcf {
                best_move: Some(stored),
                ..
            }
            | StoredAction::Vct {
                best_move: Some(stored),
                ..
            } => stored,
            _ => return None,
        };
        let best_move = canonical.move_to_original(stored);
        position.is_legal(best_move).then_some(ProofBookHit {
            best_move,
            distance: entry.distance,
        })
    }
}

fn metadata(roots: &[StoredRoot], entries: &[StoredEntry]) -> ProofBookMetadata {
    let mut sources = ProofBookSourceSummary::default();
    for entry in entries {
        match entry.action {
            StoredAction::AttackerMove(_) => sources.attacker_moves += 1,
            StoredAction::DefenderAll => sources.defender_nodes += 1,
            StoredAction::Immediate => sources.immediate_leaves += 1,
            StoredAction::Vcf { .. } => sources.vcf_leaves += 1,
            StoredAction::Vct { .. } => sources.vct_leaves += 1,
        }
    }
    ProofBookMetadata {
        version: VERSION,
        rules: RuleSet::Freestyle,
        roots: roots.len(),
        entries: entries.len(),
        black_roots: roots
            .iter()
            .filter(|root| root.attacker == Stone::Black)
            .count(),
        white_roots: roots
            .iter()
            .filter(|root| root.attacker == Stone::White)
            .count(),
        sources,
    }
}

#[derive(Debug)]
pub enum ProofBookError {
    Io(io::Error),
    Invalid(&'static str),
}

impl fmt::Display for ProofBookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "Proof Book I/O error: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid Proof Book: {message}"),
        }
    }
}

impl std::error::Error for ProofBookError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

struct Decoder<'a, R> {
    reader: &'a mut R,
}

impl<R: Read> Decoder<'_, R> {
    fn read_exact(&mut self, bytes: &mut [u8]) -> Result<(), ProofBookError> {
        self.reader.read_exact(bytes).map_err(ProofBookError::Io)
    }
    fn u8(&mut self) -> Result<u8, ProofBookError> {
        let mut bytes = [0];
        self.read_exact(&mut bytes)?;
        Ok(bytes[0])
    }
    fn u16(&mut self) -> Result<u16, ProofBookError> {
        let mut bytes = [0; 2];
        self.read_exact(&mut bytes)?;
        Ok(u16::from_le_bytes(bytes))
    }
    fn u32(&mut self) -> Result<u32, ProofBookError> {
        let mut bytes = [0; 4];
        self.read_exact(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn u64(&mut self) -> Result<u64, ProofBookError> {
        let mut bytes = [0; 8];
        self.read_exact(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn key(&mut self) -> Result<CanonicalPositionKey, ProofBookError> {
        let mut bytes = [0; CanonicalPositionKey::BYTE_LEN];
        self.read_exact(&mut bytes)?;
        CanonicalPositionKey::from_bytes(bytes)
            .map_err(|_| ProofBookError::Invalid("invalid packed canonical position"))
    }
}

fn checked_count(value: u32, maximum: usize, name: &'static str) -> Result<usize, ProofBookError> {
    let value = usize::try_from(value).map_err(|_| ProofBookError::Invalid(name))?;
    if value > maximum {
        return Err(ProofBookError::Invalid(name));
    }
    Ok(value)
}

fn write_count(writer: &mut impl Write, count: usize) -> Result<(), ProofBookError> {
    let count =
        u32::try_from(count).map_err(|_| ProofBookError::Invalid("item count too large"))?;
    writer
        .write_all(&count.to_le_bytes())
        .map_err(ProofBookError::Io)
}

fn decode_stone(tag: u8) -> Result<Stone, ProofBookError> {
    match tag {
        0 => Ok(Stone::Black),
        1 => Ok(Stone::White),
        _ => Err(ProofBookError::Invalid("invalid stone tag")),
    }
}

const fn encode_stone(stone: Stone) -> u8 {
    match stone {
        Stone::Black => 0,
        Stone::White => 1,
    }
}

fn decode_move(value: u8) -> Result<Move, ProofBookError> {
    Move::from_index(usize::from(value)).map_err(|_| ProofBookError::Invalid("invalid move index"))
}

fn decode_optional_move<R: Read>(
    decoder: &mut Decoder<'_, R>,
) -> Result<Option<Move>, ProofBookError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some(decode_move(decoder.u8()?)?)),
        _ => Err(ProofBookError::Invalid("invalid optional move tag")),
    }
}

fn encode_optional_move(writer: &mut impl Write, at: Option<Move>) -> Result<(), ProofBookError> {
    match at {
        None => writer.write_all(&[0]).map_err(ProofBookError::Io),
        Some(at) => writer
            .write_all(&[1, move_byte(at)])
            .map_err(ProofBookError::Io),
    }
}

fn validate_tactical_move(
    position: &Position,
    canonical: CanonicalPosition,
    stored: Option<Move>,
    fresh: Option<Move>,
) -> Result<(), ProofBookError> {
    let decoded = stored.map(|at| canonical.move_to_original(at));
    if decoded != fresh || decoded.is_some_and(|at| !position.is_legal(at)) {
        return Err(ProofBookError::Invalid("tactical proof move mismatch"));
    }
    Ok(())
}

fn move_byte(at: Move) -> u8 {
    u8::try_from(at.index()).unwrap_or(0)
}

fn decode_distance(tag: u8, plies: u8) -> Result<ProofDistance, ProofBookError> {
    match tag {
        0 => Ok(ProofDistance::Exact(plies)),
        1 => Ok(ProofDistance::AtMost(plies)),
        _ => Err(ProofBookError::Invalid("invalid proof distance tag")),
    }
}

pub(crate) fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<(), ProofBookError>,
) -> io::Result<()> {
    let temporary = sibling_path(path, "tmp");
    let backup = sibling_path(path, "bak");
    let mut file = File::create(&temporary)?;
    write(&mut file).map_err(io::Error::other)?;
    file.sync_all()?;
    drop(file);
    let had_previous = path.exists();
    if had_previous {
        if backup.exists() {
            fs::remove_file(&backup)?;
        }
        fs::rename(path, &backup)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if had_previous {
            let _ = fs::rename(&backup, path);
        }
        return Err(error);
    }
    if had_previous {
        fs::remove_file(backup)?;
    }
    Ok(())
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "rustmoku".into(), |name| name.to_os_string());
    name.push(format!(".{suffix}"));
    path.with_file_name(name)
}
