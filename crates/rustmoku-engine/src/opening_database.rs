//! Empirical root data. Never a proof certificate or an ordinary TT bound.
use crate::SearchProfile;
use rustmoku_core::{CanonicalPosition, CanonicalPositionKey, Move, Position};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::Path,
};
const MAGIC: &[u8; 8] = b"RMOPEN01";
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningIdentity {
    pub engine_build: String,
    pub model: Option<[u8; 32]>,
    pub profile: SearchProfile,
    pub generation: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpeningPolicy {
    OrderOnly,
    BookMove,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpeningMove {
    pub at: Move,
    /// Empirical side-to-move score, strictly outside the reserved mate domain.
    pub score: i32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningEntry {
    pub moves: Vec<OpeningMove>,
    pub depth: u8,
    pub work: u64,
    /// Explicit root/descendant/leaf domain description, never solved minimax.
    pub domain: String,
    pub wdl: Option<[u16; 3]>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningDatabase {
    identity: OpeningIdentity,
    entries: BTreeMap<CanonicalPositionKey, OpeningEntry>,
}
impl OpeningDatabase {
    pub fn new(identity: OpeningIdentity) -> io::Result<Self> {
        for text in [&identity.engine_build, &identity.generation] {
            validate_text(text)?;
        }
        Ok(Self {
            identity,
            entries: BTreeMap::new(),
        })
    }
    /// Exact immediate winning/blocking points for empirical child retention.
    pub fn protected_moves(position: &Position) -> Vec<Move> {
        let patterns = crate::PatternState::new(position);
        let side = position.side_to_move();
        patterns
            .winning_moves(side)
            .union(patterns.winning_moves(side.opponent()))
            .iter()
            .collect()
    }
    pub fn identity(&self) -> &OpeningIdentity {
        &self.identity
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Empirical starting positions only; no score or proof authority is imported.
    pub fn start_keys(&self, min_plies: usize, max_plies: usize) -> Vec<CanonicalPositionKey> {
        self.entries
            .keys()
            .copied()
            .filter(|key| {
                let count = Move::all()
                    .filter(|at| {
                        let index = at.index();
                        (key.as_bytes()[index / 4] >> (2 * (3 - index % 4))) & 3 != 0
                    })
                    .count();
                (min_plies..=max_plies).contains(&count)
            })
            .collect()
    }
    /// Construct a legal setup replay, not a reconstruction of historical play.
    /// A nonterminal Freestyle board has no winning subset, so alternating its
    /// stored stones is safe; Core still validates every move and the final key.
    pub fn replay_start(&self, key: CanonicalPositionKey) -> io::Result<rustmoku_core::Game> {
        if !self.entries.contains_key(&key) {
            return Err(invalid("unknown opening start"));
        }
        let mut stones = [Vec::new(), Vec::new()];
        for at in Move::all() {
            let index = at.index();
            let code = (key.as_bytes()[index / 4] >> (2 * (3 - index % 4))) & 3;
            if code != 0 {
                stones[usize::from(code - 1)].push(at);
            }
        }
        let mut game = rustmoku_core::Game::new(rustmoku_core::RuleSet::Freestyle);
        let count = stones[0].len() + stones[1].len();
        for ply in 0..count {
            let at = *stones[ply % 2]
                .get(ply / 2)
                .ok_or_else(|| invalid("opening stone count"))?;
            game.play_move(at)
                .map_err(|_| invalid("invalid opening setup replay"))?;
        }
        if game.position().winner().is_some()
            || game.position().is_full()
            || CanonicalPosition::new(game.position()).key() != key
        {
            return Err(invalid(
                "opening start is not a canonical nonterminal position",
            ));
        }
        Ok(game)
    }
    pub fn insert(&mut self, position: &Position, mut entry: OpeningEntry) -> io::Result<()> {
        validate_entry(&entry)?;
        if entry.moves.iter().any(|m| !position.is_legal(m.at)) {
            return Err(invalid("illegal empirical move"));
        }
        let canonical = CanonicalPosition::new(position);
        for m in &mut entry.moves {
            m.at = canonical.move_to_canonical(m.at);
        }
        entry
            .moves
            .sort_by_key(|m| (std::cmp::Reverse(m.score), m.at.index()));
        self.insert_canonical(canonical.key(), entry)
    }
    fn insert_canonical(
        &mut self,
        key: CanonicalPositionKey,
        entry: OpeningEntry,
    ) -> io::Result<()> {
        if let Some(old) = self.entries.get(&key) {
            if old != &entry {
                return Err(invalid(
                    "conflicting empirical entry; use a separate generation",
                ));
            }
        } else {
            if self.entries.len() >= MAX_ENTRIES {
                return Err(invalid("opening entry limit"));
            }
            self.entries.insert(key, entry);
        }
        Ok(())
    }
    pub fn query(&self, position: &Position, identity: &OpeningIdentity) -> Option<OpeningEntry> {
        if &self.identity != identity {
            return None;
        }
        let canonical = CanonicalPosition::new(position);
        let mut entry = self.entries.get(&canonical.key())?.clone();
        for m in &mut entry.moves {
            m.at = canonical.move_to_original(m.at);
            if !position.is_legal(m.at) {
                return None;
            }
        }
        Some(entry)
    }
    pub fn merge(&mut self, other: &Self) -> io::Result<()> {
        if self.identity != other.identity {
            return Err(invalid("opening identity mismatch"));
        }
        // Validate every conflict before changing the receiving database.
        if other
            .entries
            .iter()
            .any(|(key, value)| self.entries.get(key).is_some_and(|old| old != value))
        {
            return Err(invalid("conflicting empirical merge"));
        }
        if self
            .entries
            .keys()
            .chain(other.entries.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > MAX_ENTRIES
        {
            return Err(invalid("opening entry limit"));
        }
        self.entries.extend(other.entries.clone());
        Ok(())
    }
    pub fn read_from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::read_from(&mut std::fs::File::open(path)?)
    }
    pub fn read_from(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = Vec::new();
        reader.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(invalid("opening size limit"));
        }
        let mut r = Reader {
            bytes: &bytes,
            cursor: 0,
        };
        if r.take(8)? != MAGIC || r.byte()? != 1 {
            return Err(invalid("opening version/rule mismatch"));
        }
        let engine_build = r.text()?;
        let model = match r.byte()? {
            0 => None,
            1 => Some(r.take(32)?.try_into().unwrap()),
            _ => return Err(invalid("model tag")),
        };
        let profile = r.text()?.parse().map_err(invalid)?;
        let generation = r.text()?;
        let mut db = Self::new(OpeningIdentity {
            engine_build,
            model,
            profile,
            generation,
        })?;
        let count = u32::from_le_bytes(r.take(4)?.try_into().unwrap()) as usize;
        if count > MAX_ENTRIES {
            return Err(invalid("opening entry limit"));
        }
        for _ in 0..count {
            let key = CanonicalPositionKey::from_bytes(r.take(58)?.try_into().unwrap())
                .map_err(|_| invalid("opening key"))?;
            if db.entries.contains_key(&key) {
                return Err(invalid("duplicate opening key"));
            }
            let depth = r.byte()?;
            let work = u64::from_le_bytes(r.take(8)?.try_into().unwrap());
            let domain = r.text()?;
            let wdl = match r.byte()? {
                0 => None,
                1 => Some([r.short()?, r.short()?, r.short()?]),
                _ => return Err(invalid("WDL tag")),
            };
            let count = usize::from(r.byte()?);
            let mut moves = Vec::with_capacity(count);
            for _ in 0..count {
                let at = Move::from_index(usize::from(r.byte()?))
                    .map_err(|_| invalid("opening move"))?;
                let score = i32::from_le_bytes(r.take(4)?.try_into().unwrap());
                if (key.as_bytes()[at.index() / 4] >> (2 * (3 - at.index() % 4))) & 3 != 0 {
                    return Err(invalid("occupied opening move"));
                }
                moves.push(OpeningMove { at, score });
            }
            let entry = OpeningEntry {
                moves,
                depth,
                work,
                domain,
                wdl,
            };
            validate_entry(&entry)?;
            db.insert_canonical(key, entry)?;
        }
        if r.cursor != bytes.len() {
            return Err(invalid("trailing opening bytes"));
        }
        Ok(db)
    }
    pub fn write_to_path(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut bytes = Vec::new();
        self.write_to(&mut bytes)?;
        crate::proof_book::atomic_write(path.as_ref(), |f| {
            f.write_all(&bytes)
                .map_err(crate::proof_book::ProofBookError::Io)?;
            Ok(())
        })
    }
    pub fn write_to(&self, w: &mut impl Write) -> io::Result<()> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.push(1);
        text(&mut bytes, &self.identity.engine_build)?;
        bytes.push(u8::from(self.identity.model.is_some()));
        if let Some(model) = self.identity.model {
            bytes.extend_from_slice(&model);
        }
        text(&mut bytes, &self.identity.profile.to_string())?;
        text(&mut bytes, &self.identity.generation)?;
        bytes.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (key, e) in &self.entries {
            validate_entry(e)?;
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(e.depth);
            bytes.extend_from_slice(&e.work.to_le_bytes());
            text(&mut bytes, &e.domain)?;
            bytes.push(u8::from(e.wdl.is_some()));
            if let Some(wdl) = e.wdl {
                for value in wdl {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            bytes.push(e.moves.len() as u8);
            for m in &e.moves {
                bytes.push(m.at.index() as u8);
                bytes.extend_from_slice(&m.score.to_le_bytes());
            }
            if bytes.len() as u64 > MAX_BYTES {
                return Err(invalid("opening size limit"));
            }
        }
        w.write_all(&bytes)
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn validate_text(s: &str) -> io::Result<()> {
    if s.is_empty() || s.len() > 1024 {
        Err(invalid("opening text limit"))
    } else {
        Ok(())
    }
}
fn text(w: &mut Vec<u8>, s: &str) -> io::Result<()> {
    validate_text(s)?;
    w.extend_from_slice(&(s.len() as u16).to_le_bytes());
    w.extend_from_slice(s.as_bytes());
    Ok(())
}
fn validate_entry(e: &OpeningEntry) -> io::Result<()> {
    validate_text(&e.domain)?;
    let mut seen = std::collections::BTreeSet::new();
    if e.depth == 0
        || e.moves.is_empty()
        || e.moves.len() > 225
        || e.moves
            .iter()
            .any(|m| m.score.unsigned_abs() > 10_000_000 || !seen.insert(m.at.index()))
        || e.wdl
            .is_some_and(|w| w.into_iter().map(u32::from).sum::<u32>() != 32768)
    {
        return Err(invalid("invalid empirical entry"));
    }
    Ok(())
}
struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        let end = self
            .cursor
            .checked_add(n)
            .ok_or_else(|| invalid("length overflow"))?;
        let result = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| invalid("truncated opening database"))?;
        self.cursor = end;
        Ok(result)
    }
    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn short(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn text(&mut self) -> io::Result<String> {
        let n = usize::from(self.short()?);
        if n == 0 || n > 1024 {
            return Err(invalid("text limit"));
        }
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| invalid("invalid UTF8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AlphaBetaEngine, EngineConfig, PatternEvaluator, ScoreContract, SearchEngine, SearchLimits,
        SearchOrigin,
    };
    use rustmoku_core::{Game, RuleSet, Symmetry};
    use std::sync::Arc;
    fn identity() -> OpeningIdentity {
        OpeningIdentity {
            engine_build: "test-build".into(),
            model: crate::Evaluator::model_fingerprint(&PatternEvaluator),
            profile: crate::SearchProfile::baseline(ScoreContract::Pattern),
            generation: "fixture-depth2".into(),
        }
    }
    fn game() -> Game {
        let mut g = Game::new(RuleSet::Freestyle);
        for i in [112, 97, 128, 113] {
            g.play_move(Move::from_index(i).unwrap()).unwrap();
        }
        g
    }
    #[test]
    fn d4_query_dedupe_identity_and_roundtrip_are_empirical() {
        let g = game();
        let at = Move::from_index(0).unwrap();
        let mut db = OpeningDatabase::new(identity()).unwrap();
        let entry = OpeningEntry {
            moves: vec![OpeningMove { at, score: 123 }],
            depth: 2,
            work: 10,
            domain: "all-legal/production-radius-two/four-q6".into(),
            wdl: None,
        };
        db.insert(g.position(), entry.clone()).unwrap();
        let mut transformed = Game::new(RuleSet::Freestyle);
        for m in g.history() {
            transformed
                .play_move(Symmetry::Rotate90.transform(m))
                .unwrap();
        }
        let mut rotated = entry;
        rotated.moves[0].at = Symmetry::Rotate90.transform(at);
        db.insert(transformed.position(), rotated).unwrap();
        assert_eq!(db.len(), 1);
        let keys = db.start_keys(2, 16);
        assert_eq!(keys.len(), 1);
        let replayed = db.replay_start(keys[0]).unwrap();
        assert_eq!(CanonicalPosition::new(replayed.position()).key(), keys[0]);
        assert_eq!(replayed.position().move_count(), g.position().move_count());
        assert!(db.start_keys(5, 16).is_empty());
        assert_eq!(
            db.query(transformed.position(), &identity()).unwrap().moves[0].at,
            Symmetry::Rotate90.transform(at)
        );
        let mut wrong = identity();
        wrong.engine_build = "wrong".into();
        assert!(db.query(g.position(), &wrong).is_none());
        let mut bytes = Vec::new();
        db.write_to(&mut bytes).unwrap();
        assert_eq!(db, OpeningDatabase::read_from(&mut &bytes[..]).unwrap());
        assert!(OpeningDatabase::read_from(&mut &bytes[..bytes.len() - 1]).is_err());
        let mut merged = OpeningDatabase::new(identity()).unwrap();
        merged.merge(&db).unwrap();
        merged.merge(&db).unwrap();
        assert_eq!(merged, db);
    }
    #[test]
    fn book_move_has_no_proof_or_tt_authority_and_order_only_searches() {
        let g = game();
        let mut db = OpeningDatabase::new(identity()).unwrap();
        db.insert(
            g.position(),
            OpeningEntry {
                moves: vec![OpeningMove {
                    at: Move::from_index(0).unwrap(),
                    score: 123,
                }],
                depth: 2,
                work: 10,
                domain: "declared-horizon".into(),
                wdl: None,
            },
        )
        .unwrap();
        let db = Arc::new(db);
        let mut e = AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1));
        assert!(
            e.set_opening_database(db.clone(), OpeningPolicy::BookMove, "wrong")
                .is_err()
        );
        e.set_opening_database(db.clone(), OpeningPolicy::BookMove, "test-build")
            .unwrap();
        let result = e.search(g.position(), SearchLimits::new(2));
        assert_eq!(result.origin, SearchOrigin::OpeningBook);
        assert!(result.proof.is_none());
        assert_eq!(result.completed_depth, 0);
        assert_eq!(e.transposition_table_statistics().hashfull_per_mille, 0);
        e.set_opening_database(db, OpeningPolicy::OrderOnly, "test-build")
            .unwrap();
        let result = e.search(g.position(), SearchLimits::new(1));
        assert_eq!(result.origin, SearchOrigin::AlphaBeta);
        assert_eq!(result.completed_depth, 1);
    }
}
