//! Deterministic offline AND/OR proof-number solving and resumable state.
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::File,
    io::{self, Read, Write},
    path::Path,
    time::{Duration, Instant},
};

use rustmoku_core::{
    BOARD_SIZE, CELL_COUNT, CanonicalPosition, CanonicalPositionKey, Game, Move, Position, RuleSet,
    Stone,
};

use crate::{
    CancellationToken, ProofLimits, SearchLimits,
    board_state::BoardState,
    proof_book::{
        EntryKey, ProofBook, ProofDistance, StoneKey, StoredAction, StoredEntry, StoredRoot,
        atomic_write,
    },
    search_control::SearchBudget,
    tactical::{ImmediateTactic, forcing_moves, immediate_tactic},
    vcf::{VcfSolver, VcfStatus},
    vct::{VctSolver, VctStatus, attacks},
};

const INFINITY: u64 = u64::MAX / 4;
const WIDEN_BATCH: usize = 4;
const CHECKPOINT_MAGIC: &[u8; 8] = b"RMPCHK01";
const CHECKPOINT_VERSION: u16 = 1;
const MAX_CHECKPOINT_NODES: usize = 100_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProofOutcome {
    ProvenWin,
    Refuted,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SolverLimits {
    pub max_work_nodes: u64,
    pub max_duration: Option<Duration>,
    pub max_resident_nodes: Option<usize>,
    pub vcf: ProofLimits,
    pub vct: ProofLimits,
}

impl SolverLimits {
    #[must_use]
    pub const fn new(max_work_nodes: u64) -> Self {
        Self {
            max_work_nodes,
            max_duration: None,
            max_resident_nodes: None,
            vcf: ProofLimits::new(7, 1_000),
            vct: ProofLimits::new(7, 2_000),
        }
    }

    #[must_use]
    pub const fn with_duration(mut self, duration: Duration) -> Self {
        self.max_duration = Some(duration);
        self
    }

    #[must_use]
    pub const fn with_max_resident_nodes(mut self, nodes: usize) -> Self {
        self.max_resident_nodes = Some(nodes);
        self
    }

    #[must_use]
    pub const fn with_vcf(mut self, limits: ProofLimits) -> Self {
        self.vcf = limits;
        self
    }

    #[must_use]
    pub const fn with_vct(mut self, limits: ProofLimits) -> Self {
        self.vct = limits;
        self
    }
}

impl Default for SolverLimits {
    fn default() -> Self {
        Self::new(100_000)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SolverStatistics {
    pub work_nodes: u64,
    pub expanded_nodes: u64,
    pub generated_children: u64,
    pub exact_cache_hits: u64,
    pub vcf_attempts: u64,
    pub vcf_proven: u64,
    pub vct_attempts: u64,
    pub vct_proven: u64,
    pub progressive_widen_events: u64,
    pub resident_nodes: usize,
    pub unresolved_nodes: usize,
    pub root_proof_number: u64,
    pub root_disproof_number: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SolverTermination {
    #[default]
    Complete,
    WorkLimit,
    TimeLimit,
    Cancelled,
    ResidentLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SolverResult {
    pub outcome: ProofOutcome,
    pub termination: SolverTermination,
    pub statistics: SolverStatistics,
}

#[derive(Debug)]
pub enum SolverError {
    Io(io::Error),
    Invalid(&'static str),
    Incomplete,
}

impl fmt::Display for SolverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "offline solver I/O error: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid solver state: {message}"),
            Self::Incomplete => formatter.write_str("the root is not a proven win"),
        }
    }
}

impl std::error::Error for SolverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) | Self::Incomplete => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Evidence {
    Unknown,
    Terminal,
    Immediate,
    Vcf { max_plies: u8, max_nodes: u64 },
    Vct { max_plies: u8, max_nodes: u64 },
    Aggregated,
    Cached(usize),
}

#[derive(Debug)]
struct Child {
    at: Move,
    node: usize,
}

#[derive(Debug)]
struct Node {
    parent: Option<usize>,
    via: Option<Move>,
    position: Position,
    ordered_moves: Vec<Move>,
    next_unexpanded: usize,
    children: Vec<Child>,
    oracle_done: bool,
    outcome: ProofOutcome,
    proof: u64,
    disproof: u64,
    evidence: Evidence,
}

impl Node {
    fn new(parent: Option<usize>, via: Option<Move>, position: Position) -> Self {
        Self {
            parent,
            via,
            ordered_moves: ordered_legal_moves(&position),
            next_unexpanded: 0,
            children: Vec::new(),
            oracle_done: false,
            position,
            outcome: ProofOutcome::Unknown,
            proof: 1,
            disproof: 1,
            evidence: Evidence::Unknown,
        }
    }

    fn set_outcome(&mut self, outcome: ProofOutcome) {
        self.outcome = outcome;
        (self.proof, self.disproof) = match outcome {
            ProofOutcome::ProvenWin => (0, INFINITY),
            ProofOutcome::Refuted => (INFINITY, 0),
            ProofOutcome::Unknown => (1, 1),
        };
    }
}

#[derive(Clone, Copy, Debug)]
struct Cached {
    outcome: ProofOutcome,
    source: Option<usize>,
}

/// Single-thread deterministic proof-number manager for one fixed attacker.
pub struct OfflineSolver {
    attacker: Stone,
    root_moves: Vec<Move>,
    nodes: Vec<Node>,
    exact: BTreeMap<EntryKey, Cached>,
    statistics: SolverStatistics,
}

impl OfflineSolver {
    pub fn new(game: &Game, attacker: Stone) -> Result<Self, SolverError> {
        if game.position().rules() != RuleSet::Freestyle {
            return Err(SolverError::Invalid("only Freestyle is supported"));
        }
        let root_moves: Vec<_> = game.history().collect();
        let mut root = Node::new(None, None, game.position().clone());
        apply_terminal(&mut root, attacker);
        let mut solver = Self {
            attacker,
            root_moves,
            nodes: vec![root],
            exact: BTreeMap::new(),
            statistics: SolverStatistics::default(),
        };
        solver.cache_if_exact(0);
        solver.refresh_statistics();
        Ok(solver)
    }

    #[must_use]
    pub const fn attacker(&self) -> Stone {
        self.attacker
    }

    #[must_use]
    pub fn root_position(&self) -> &Position {
        &self.nodes[0].position
    }

    #[must_use]
    pub fn root_moves(&self) -> &[Move] {
        &self.root_moves
    }

    #[must_use]
    pub const fn statistics(&self) -> SolverStatistics {
        self.statistics
    }

    pub fn solve(&mut self, limits: SolverLimits) -> SolverResult {
        self.solve_controlled(limits, CancellationToken::new())
    }

    pub fn solve_controlled(
        &mut self,
        limits: SolverLimits,
        cancellation: CancellationToken,
    ) -> SolverResult {
        let start = Instant::now();
        let initial_work = self.statistics.work_nodes;
        let mut termination = SolverTermination::Complete;
        while self.nodes[0].outcome == ProofOutcome::Unknown {
            termination = if cancellation.is_cancelled() {
                SolverTermination::Cancelled
            } else if limits
                .max_duration
                .is_some_and(|duration| start.elapsed() >= duration)
            {
                SolverTermination::TimeLimit
            } else if self.statistics.work_nodes.saturating_sub(initial_work)
                >= limits.max_work_nodes
            {
                SolverTermination::WorkLimit
            } else if limits
                .max_resident_nodes
                .is_some_and(|maximum| self.nodes.len() >= maximum)
            {
                SolverTermination::ResidentLimit
            } else {
                let selected = self.select_most_proving(0);
                self.expand_selected(selected, limits, &cancellation, start, initial_work);
                self.propagate(selected);
                continue;
            };
            break;
        }
        self.refresh_statistics();
        SolverResult {
            outcome: self.nodes[0].outcome,
            termination: if self.nodes[0].outcome == ProofOutcome::Unknown {
                termination
            } else {
                SolverTermination::Complete
            },
            statistics: self.statistics,
        }
    }

    fn select_most_proving(&self, mut id: usize) -> usize {
        loop {
            let node = &self.nodes[id];
            if !node.oracle_done {
                return id;
            }
            let is_or = node.position.side_to_move() == self.attacker;
            let actual = node
                .children
                .iter()
                .filter(|child| self.nodes[child.node].outcome == ProofOutcome::Unknown)
                .min_by_key(|child| {
                    let child_node = &self.nodes[child.node];
                    (
                        if is_or {
                            child_node.proof
                        } else {
                            child_node.disproof
                        },
                        child.at.index(),
                    )
                });
            if node.next_unexpanded < node.ordered_moves.len() {
                let virtual_move = node.ordered_moves[node.next_unexpanded];
                let choose_virtual = actual.is_none_or(|child| {
                    let child_node = &self.nodes[child.node];
                    (1, virtual_move.index())
                        <= (
                            if is_or {
                                child_node.proof
                            } else {
                                child_node.disproof
                            },
                            child.at.index(),
                        )
                });
                if choose_virtual {
                    return id;
                }
            }
            let Some(next) = actual else {
                return id;
            };
            id = next.node;
        }
    }

    fn expand_selected(
        &mut self,
        id: usize,
        limits: SolverLimits,
        cancellation: &CancellationToken,
        start: Instant,
        initial_work: u64,
    ) {
        if self.nodes[id].outcome != ProofOutcome::Unknown {
            return;
        }
        if !self.nodes[id].oracle_done {
            self.nodes[id].oracle_done = true;
            self.statistics.expanded_nodes += 1;
            self.statistics.work_nodes += 1;
            if self.apply_immediate(id) {
                self.cache_if_exact(id);
                return;
            }
            if self.try_tactical(id, limits, cancellation, start, initial_work) {
                self.cache_if_exact(id);
                return;
            }
        }
        if self.nodes[id].next_unexpanded >= self.nodes[id].ordered_moves.len() {
            self.recompute(id);
            return;
        }
        self.statistics.progressive_widen_events += 1;
        for _ in 0..WIDEN_BATCH {
            if self.statistics.work_nodes.saturating_sub(initial_work) >= limits.max_work_nodes
                || cancellation.is_cancelled()
                || limits
                    .max_duration
                    .is_some_and(|limit| start.elapsed() >= limit)
                || limits
                    .max_resident_nodes
                    .is_some_and(|maximum| self.nodes.len() >= maximum)
                || self.nodes[id].next_unexpanded >= self.nodes[id].ordered_moves.len()
            {
                break;
            }
            let at = self.nodes[id].ordered_moves[self.nodes[id].next_unexpanded];
            self.nodes[id].next_unexpanded += 1;
            let mut child_position = self.nodes[id].position.clone();
            if child_position.make_move(at).is_err() {
                continue;
            }
            let key = context_key(&child_position, self.attacker);
            let cached = self.exact.get(&key).copied();
            let mut child = Node::new(Some(id), Some(at), child_position);
            apply_terminal(&mut child, self.attacker);
            if child.outcome == ProofOutcome::Unknown
                && let Some(cached) = cached
            {
                child.set_outcome(cached.outcome);
                child.evidence = cached.source.map_or(Evidence::Terminal, Evidence::Cached);
                self.statistics.exact_cache_hits += 1;
            }
            let child_id = self.nodes.len();
            self.nodes.push(child);
            self.nodes[id].children.push(Child { at, node: child_id });
            self.statistics.generated_children += 1;
            self.statistics.work_nodes += 1;
            self.cache_if_exact(child_id);
        }
        self.recompute(id);
    }

    fn apply_immediate(&mut self, id: usize) -> bool {
        let Some(outcome) = immediate_exact_outcome(&self.nodes[id].position, self.attacker) else {
            return false;
        };
        self.nodes[id].set_outcome(outcome);
        self.nodes[id].evidence = Evidence::Immediate;
        true
    }

    fn try_tactical(
        &mut self,
        id: usize,
        limits: SolverLimits,
        cancellation: &CancellationToken,
        start: Instant,
        initial_work: u64,
    ) -> bool {
        let remaining = limits
            .max_work_nodes
            .saturating_sub(self.statistics.work_nodes.saturating_sub(initial_work));
        if remaining == 0 {
            return false;
        }
        let remaining_time = limits
            .max_duration
            .and_then(|limit| limit.checked_sub(start.elapsed()));
        if limits.vcf.enabled() {
            let board = BoardState::new(&self.nodes[id].position);
            if !forcing_moves(board.patterns(), self.attacker).is_empty() {
                self.statistics.vcf_attempts += 1;
                let (proof, used) = tactical_attempt(
                    &self.nodes[id].position,
                    self.attacker,
                    false,
                    limits.vcf,
                    remaining,
                    remaining_time,
                    cancellation.clone(),
                );
                self.statistics.work_nodes += used;
                if proof.is_some() {
                    self.statistics.vcf_proven += 1;
                    self.nodes[id].set_outcome(ProofOutcome::ProvenWin);
                    self.nodes[id].evidence = Evidence::Vcf {
                        max_plies: limits.vcf.max_plies,
                        max_nodes: limits.vcf.max_nodes,
                    };
                    return true;
                }
            }
        }
        let remaining = limits
            .max_work_nodes
            .saturating_sub(self.statistics.work_nodes.saturating_sub(initial_work));
        if remaining == 0 {
            return false;
        }
        if limits.vct.enabled() {
            let board = BoardState::new(&self.nodes[id].position);
            if !attacks(board.patterns(), self.attacker).is_empty() {
                self.statistics.vct_attempts += 1;
                let remaining_time = limits
                    .max_duration
                    .and_then(|limit| limit.checked_sub(start.elapsed()));
                let (proof, used) = tactical_attempt(
                    &self.nodes[id].position,
                    self.attacker,
                    true,
                    limits.vct,
                    remaining,
                    remaining_time,
                    cancellation.clone(),
                );
                self.statistics.work_nodes += used;
                if proof.is_some() {
                    self.statistics.vct_proven += 1;
                    self.nodes[id].set_outcome(ProofOutcome::ProvenWin);
                    self.nodes[id].evidence = Evidence::Vct {
                        max_plies: limits.vct.max_plies,
                        max_nodes: limits.vct.max_nodes,
                    };
                    return true;
                }
            }
        }
        false
    }

    fn recompute(&mut self, id: usize) {
        if self.nodes[id].outcome != ProofOutcome::Unknown
            && self.nodes[id].evidence != Evidence::Aggregated
        {
            return;
        }
        let is_or = self.nodes[id].position.side_to_move() == self.attacker;
        let omitted = self.nodes[id].ordered_moves.len() - self.nodes[id].next_unexpanded;
        let mut proof = if is_or { INFINITY } else { 0 };
        let mut disproof = if is_or { 0 } else { INFINITY };
        for child in &self.nodes[id].children {
            let child = &self.nodes[child.node];
            if is_or {
                proof = proof.min(child.proof);
                disproof = saturated_add(disproof, child.disproof);
            } else {
                proof = saturated_add(proof, child.proof);
                disproof = disproof.min(child.disproof);
            }
        }
        if omitted != 0 {
            if is_or {
                proof = proof.min(1);
                disproof = saturated_add(disproof, omitted as u64);
            } else {
                proof = saturated_add(proof, omitted as u64);
                disproof = disproof.min(1);
            }
        }
        self.nodes[id].proof = proof;
        self.nodes[id].disproof = disproof;
        self.nodes[id].outcome = if proof == 0 {
            ProofOutcome::ProvenWin
        } else if disproof == 0 {
            ProofOutcome::Refuted
        } else {
            ProofOutcome::Unknown
        };
        self.nodes[id].evidence = if self.nodes[id].outcome == ProofOutcome::Unknown {
            Evidence::Unknown
        } else {
            Evidence::Aggregated
        };
        self.cache_if_exact(id);
    }

    fn propagate(&mut self, mut id: usize) {
        loop {
            self.recompute(id);
            let Some(parent) = self.nodes[id].parent else {
                break;
            };
            id = parent;
        }
    }

    fn cache_if_exact(&mut self, id: usize) {
        let outcome = self.nodes[id].outcome;
        if outcome == ProofOutcome::Unknown {
            return;
        }
        let key = context_key(&self.nodes[id].position, self.attacker);
        self.exact.entry(key).or_insert(Cached {
            outcome,
            source: (outcome == ProofOutcome::ProvenWin).then_some(id),
        });
    }

    fn refresh_statistics(&mut self) {
        self.statistics.resident_nodes = self.nodes.len();
        self.statistics.unresolved_nodes = self
            .nodes
            .iter()
            .filter(|node| node.outcome == ProofOutcome::Unknown)
            .count();
        self.statistics.root_proof_number = self.nodes[0].proof;
        self.statistics.root_disproof_number = self.nodes[0].disproof;
    }

    pub fn export_proof_book(&self) -> Result<ProofBook, SolverError> {
        if self.nodes[0].outcome != ProofOutcome::ProvenWin {
            return Err(SolverError::Incomplete);
        }
        let mut entries = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        self.export_node(0, &mut entries, &mut visiting)?;
        let root = StoredRoot {
            attacker: self.attacker,
            moves: self.root_moves.clone(),
            key: CanonicalPosition::new(&self.nodes[0].position).key(),
        };
        Ok(ProofBook::new(vec![root], entries.into_values().collect()))
    }

    fn export_node(
        &self,
        id: usize,
        entries: &mut BTreeMap<EntryKey, StoredEntry>,
        visiting: &mut BTreeSet<usize>,
    ) -> Result<ProofDistance, SolverError> {
        if !visiting.insert(id) {
            return Err(SolverError::Invalid("cycle in solved tree"));
        }
        let node = &self.nodes[id];
        if node.position.winner() == Some(self.attacker) {
            visiting.remove(&id);
            return Ok(ProofDistance::Exact(0));
        }
        if node.outcome != ProofOutcome::ProvenWin {
            return Err(SolverError::Invalid("unproven node in strategy"));
        }
        if let Evidence::Cached(source) = node.evidence {
            let distance = self.export_node(source, entries, visiting)?;
            visiting.remove(&id);
            return Ok(distance);
        }
        let canonical = CanonicalPosition::new(&node.position);
        let key = context_key(&node.position, self.attacker);
        let (action, distance) = match node.evidence {
            Evidence::Immediate if node.position.side_to_move() == self.attacker => {
                let board = BoardState::new(&node.position);
                let ImmediateTactic::Win(at) = immediate_tactic(board.patterns(), self.attacker)
                else {
                    return Err(SolverError::Invalid("invalid attacker immediate evidence"));
                };
                (
                    StoredAction::AttackerMove(canonical.move_to_canonical(at)),
                    ProofDistance::AtMost(1),
                )
            }
            Evidence::Immediate => (
                StoredAction::Immediate,
                ProofDistance::Exact(
                    verify_immediate(&node.position, self.attacker)
                        .ok_or(SolverError::Invalid("invalid immediate evidence"))?,
                ),
            ),
            Evidence::Vcf {
                max_plies,
                max_nodes,
            } => {
                let (plies, best_move) = verify_tactical_line(
                    &node.position,
                    self.attacker,
                    false,
                    max_plies,
                    max_nodes,
                )
                .ok_or(SolverError::Invalid("VCF evidence no longer verifies"))?;
                (
                    StoredAction::Vcf {
                        best_move: best_move.map(|at| canonical.move_to_canonical(at)),
                        max_plies,
                        max_nodes,
                    },
                    ProofDistance::AtMost(plies),
                )
            }
            Evidence::Vct {
                max_plies,
                max_nodes,
            } => {
                let (plies, best_move) =
                    verify_tactical_line(&node.position, self.attacker, true, max_plies, max_nodes)
                        .ok_or(SolverError::Invalid("VCT evidence no longer verifies"))?;
                (
                    StoredAction::Vct {
                        best_move: best_move.map(|at| canonical.move_to_canonical(at)),
                        max_plies,
                        max_nodes,
                    },
                    ProofDistance::AtMost(plies),
                )
            }
            Evidence::Aggregated if node.position.side_to_move() == self.attacker => {
                let child = node
                    .children
                    .iter()
                    .filter(|child| self.nodes[child.node].outcome == ProofOutcome::ProvenWin)
                    .min_by_key(|child| canonical.move_to_canonical(child.at).index())
                    .ok_or(SolverError::Invalid("OR proof has no proven child"))?;
                let child_distance = self.export_node(child.node, entries, visiting)?;
                (
                    StoredAction::AttackerMove(canonical.move_to_canonical(child.at)),
                    ProofDistance::AtMost(child_distance.plies().saturating_add(1)),
                )
            }
            Evidence::Aggregated => {
                if node.next_unexpanded != node.ordered_moves.len()
                    || node.children.len() != node.ordered_moves.len()
                {
                    return Err(SolverError::Invalid("partial AND node marked proven"));
                }
                let mut longest = 0;
                for child in &node.children {
                    let distance = self.export_node(child.node, entries, visiting)?;
                    longest = longest.max(distance.plies());
                }
                (
                    StoredAction::DefenderAll,
                    ProofDistance::AtMost(longest.saturating_add(1)),
                )
            }
            Evidence::Terminal if node.position.winner() == Some(self.attacker) => {
                visiting.remove(&id);
                return Ok(ProofDistance::Exact(0));
            }
            Evidence::Unknown | Evidence::Terminal | Evidence::Cached(_) => {
                return Err(SolverError::Invalid("invalid winning evidence"));
            }
        };
        let entry = StoredEntry {
            key,
            distance,
            action,
        };
        if let Some(previous) = entries.insert(key, entry)
            && previous != entry
        {
            return Err(SolverError::Invalid("conflicting canonical strategies"));
        }
        visiting.remove(&id);
        Ok(distance)
    }

    pub fn save_checkpoint(&self, path: impl AsRef<Path>) -> Result<(), SolverError> {
        atomic_write(path.as_ref(), |file| {
            self.write_checkpoint(file).map_err(|error| match error {
                SolverError::Io(error) => crate::ProofBookError::Io(error),
                SolverError::Invalid(message) => crate::ProofBookError::Invalid(message),
                SolverError::Incomplete => crate::ProofBookError::Invalid("incomplete checkpoint"),
            })
        })
        .map_err(SolverError::Io)
    }

    fn write_checkpoint(&self, writer: &mut impl Write) -> Result<(), SolverError> {
        writer
            .write_all(CHECKPOINT_MAGIC)
            .map_err(SolverError::Io)?;
        writer
            .write_all(&CHECKPOINT_VERSION.to_le_bytes())
            .map_err(SolverError::Io)?;
        writer.write_all(&[0]).map_err(SolverError::Io)?;
        writer
            .write_all(&[stone_byte(self.attacker)])
            .map_err(SolverError::Io)?;
        write_u16(writer, self.root_moves.len())?;
        for &at in &self.root_moves {
            writer
                .write_all(&[move_byte(at)])
                .map_err(SolverError::Io)?;
        }
        writer
            .write_all(
                CanonicalPosition::new(&self.nodes[0].position)
                    .key()
                    .as_bytes(),
            )
            .map_err(SolverError::Io)?;
        for value in statistics_values(self.statistics) {
            writer
                .write_all(&value.to_le_bytes())
                .map_err(SolverError::Io)?;
        }
        let count = u32::try_from(self.nodes.len())
            .map_err(|_| SolverError::Invalid("too many checkpoint nodes"))?;
        writer
            .write_all(&count.to_le_bytes())
            .map_err(SolverError::Io)?;
        for node in &self.nodes {
            write_optional_index(writer, node.parent)?;
            write_optional_move(writer, node.via)?;
            writer
                .write_all(&[outcome_byte(node.outcome), u8::from(node.oracle_done)])
                .map_err(SolverError::Io)?;
            write_u16(writer, node.next_unexpanded)?;
            writer
                .write_all(&node.proof.to_le_bytes())
                .map_err(SolverError::Io)?;
            writer
                .write_all(&node.disproof.to_le_bytes())
                .map_err(SolverError::Io)?;
            write_evidence(writer, node.evidence)?;
        }
        Ok(())
    }

    pub fn load_checkpoint(path: impl AsRef<Path>) -> Result<Self, SolverError> {
        let mut file = File::open(path).map_err(SolverError::Io)?;
        let mut decoder = CheckpointDecoder { reader: &mut file };
        let mut magic = [0; 8];
        decoder.exact(&mut magic)?;
        if &magic != CHECKPOINT_MAGIC || decoder.u16()? != CHECKPOINT_VERSION {
            return Err(SolverError::Invalid("checkpoint magic or version mismatch"));
        }
        if decoder.u8()? != 0 {
            return Err(SolverError::Invalid("checkpoint rules mismatch"));
        }
        let attacker = decode_stone(decoder.u8()?)?;
        let root_count = usize::from(decoder.u16()?);
        if root_count > CELL_COUNT {
            return Err(SolverError::Invalid("too many root moves"));
        }
        let mut root_moves = Vec::with_capacity(root_count);
        let mut game = Game::new(RuleSet::Freestyle);
        for _ in 0..root_count {
            let at = decode_move(decoder.u8()?)?;
            game.play_move(at)
                .map_err(|_| SolverError::Invalid("illegal checkpoint root"))?;
            root_moves.push(at);
        }
        let stored_root = decoder.key()?;
        if stored_root != CanonicalPosition::new(game.position()).key() {
            return Err(SolverError::Invalid("checkpoint root identity mismatch"));
        }
        let mut values = Vec::with_capacity(STAT_COUNT);
        for _ in 0..STAT_COUNT {
            values.push(decoder.u64()?);
        }
        let mut statistics = statistics_from_values(&values)?;
        let node_count = usize::try_from(decoder.u32()?)
            .map_err(|_| SolverError::Invalid("checkpoint node count"))?;
        if node_count == 0 || node_count > MAX_CHECKPOINT_NODES {
            return Err(SolverError::Invalid("checkpoint node count"));
        }
        let mut nodes: Vec<Node> = Vec::with_capacity(node_count);
        for id in 0..node_count {
            let parent = decoder.optional_index()?;
            let via = decoder.optional_move()?;
            let outcome = decode_outcome(decoder.u8()?)?;
            let oracle_done = decode_bool(decoder.u8()?)?;
            let next_unexpanded = usize::from(decoder.u16()?);
            let proof = decoder.u64()?;
            let disproof = decoder.u64()?;
            let evidence = decoder.evidence()?;
            if proof > INFINITY || disproof > INFINITY {
                return Err(SolverError::Invalid("checkpoint proof number overflow"));
            }
            let position = if id == 0 {
                if parent.is_some() || via.is_some() {
                    return Err(SolverError::Invalid("checkpoint root has a parent"));
                }
                game.position().clone()
            } else {
                let parent = parent
                    .filter(|&parent| parent < id)
                    .ok_or(SolverError::Invalid("invalid checkpoint parent"))?;
                let at = via.ok_or(SolverError::Invalid("checkpoint child has no move"))?;
                let mut position = nodes[parent].position.clone();
                position
                    .make_move(at)
                    .map_err(|_| SolverError::Invalid("illegal checkpoint transition"))?;
                position
            };
            let mut node = Node::new(parent, via, position);
            if next_unexpanded > node.ordered_moves.len() {
                return Err(SolverError::Invalid("checkpoint expansion cursor"));
            }
            node.next_unexpanded = next_unexpanded;
            node.oracle_done = oracle_done;
            node.outcome = outcome;
            node.proof = proof;
            node.disproof = disproof;
            node.evidence = evidence;
            nodes.push(node);
            if let Some(parent) = parent {
                let at = via.ok_or(SolverError::Invalid("checkpoint child has no move"))?;
                if nodes[parent].children.iter().any(|child| child.at == at) {
                    return Err(SolverError::Invalid("duplicate checkpoint child"));
                }
                nodes[parent].children.push(Child { at, node: id });
            }
        }
        let mut trailing = [0];
        if decoder
            .reader
            .read(&mut trailing)
            .map_err(SolverError::Io)?
            != 0
        {
            return Err(SolverError::Invalid("trailing checkpoint bytes"));
        }
        let mut solver = Self {
            attacker,
            root_moves,
            nodes,
            exact: BTreeMap::new(),
            statistics,
        };
        for (id, node) in solver.nodes.iter().enumerate() {
            if node.children.len() != node.next_unexpanded
                || !node
                    .children
                    .iter()
                    .zip(node.ordered_moves.iter())
                    .all(|(child, expected)| child.at == *expected)
            {
                return Err(SolverError::Invalid("checkpoint child expansion order"));
            }
            match node.evidence {
                Evidence::Unknown if node.outcome != ProofOutcome::Unknown => {
                    return Err(SolverError::Invalid("unknown evidence has exact outcome"));
                }
                Evidence::Terminal => {
                    let expected = if node.position.winner() == Some(attacker) {
                        ProofOutcome::ProvenWin
                    } else if node.position.winner().is_some() || node.position.is_full() {
                        ProofOutcome::Refuted
                    } else {
                        return Err(SolverError::Invalid("nonterminal terminal evidence"));
                    };
                    if node.outcome != expected {
                        return Err(SolverError::Invalid("terminal outcome mismatch"));
                    }
                }
                Evidence::Immediate
                    if immediate_exact_outcome(&node.position, attacker) != Some(node.outcome) =>
                {
                    return Err(SolverError::Invalid("immediate outcome mismatch"));
                }
                Evidence::Vcf { .. } | Evidence::Vct { .. }
                    if node.outcome != ProofOutcome::ProvenWin =>
                {
                    return Err(SolverError::Invalid("tactical leaf is not a proven win"));
                }
                Evidence::Cached(source) => {
                    if source >= id {
                        return Err(SolverError::Invalid("checkpoint cache is not backward"));
                    }
                    let source = solver
                        .nodes
                        .get(source)
                        .ok_or(SolverError::Invalid("checkpoint cache source"))?;
                    if source.outcome != node.outcome
                        || context_key(&source.position, attacker)
                            != context_key(&node.position, attacker)
                    {
                        return Err(SolverError::Invalid("checkpoint cache mismatch"));
                    }
                }
                _ => {}
            }
            let expected_numbers = match node.outcome {
                ProofOutcome::ProvenWin => Some((0, INFINITY)),
                ProofOutcome::Refuted => Some((INFINITY, 0)),
                ProofOutcome::Unknown => None,
            };
            if expected_numbers.is_some_and(|expected| expected != (node.proof, node.disproof)) {
                return Err(SolverError::Invalid("checkpoint outcome numbers"));
            }
            if id == 0 && (node.parent.is_some() || node.via.is_some()) {
                return Err(SolverError::Invalid("checkpoint root linkage"));
            }
        }
        for id in (0..solver.nodes.len()).rev() {
            let stored = (
                solver.nodes[id].outcome,
                solver.nodes[id].proof,
                solver.nodes[id].disproof,
            );
            if solver.nodes[id].evidence == Evidence::Aggregated
                || (solver.nodes[id].evidence == Evidence::Unknown && solver.nodes[id].oracle_done)
            {
                solver.recompute(id);
            }
            if stored
                != (
                    solver.nodes[id].outcome,
                    solver.nodes[id].proof,
                    solver.nodes[id].disproof,
                )
            {
                return Err(SolverError::Invalid(
                    "checkpoint proof numbers are inconsistent",
                ));
            }
        }
        solver.exact.clear();
        for id in 0..solver.nodes.len() {
            solver.cache_if_exact(id);
        }
        statistics.resident_nodes = solver.nodes.len();
        solver.statistics = statistics;
        solver.refresh_statistics();
        Ok(solver)
    }
}

fn apply_terminal(node: &mut Node, attacker: Stone) {
    if let Some(winner) = node.position.winner() {
        node.set_outcome(if winner == attacker {
            ProofOutcome::ProvenWin
        } else {
            ProofOutcome::Refuted
        });
        node.evidence = Evidence::Terminal;
    } else if node.position.is_full() {
        node.set_outcome(ProofOutcome::Refuted);
        node.evidence = Evidence::Terminal;
    }
}

fn immediate_exact_outcome(position: &Position, attacker: Stone) -> Option<ProofOutcome> {
    if position.winner().is_some() || position.is_full() {
        return None;
    }
    let board = BoardState::new(position);
    let side = position.side_to_move();
    match (side == attacker, immediate_tactic(board.patterns(), side)) {
        (true, ImmediateTactic::Win(_)) | (false, ImmediateTactic::Loss { .. }) => {
            Some(ProofOutcome::ProvenWin)
        }
        (true, ImmediateTactic::Loss { .. }) | (false, ImmediateTactic::Win(_)) => {
            Some(ProofOutcome::Refuted)
        }
        _ => None,
    }
}

fn context_key(position: &Position, attacker: Stone) -> EntryKey {
    EntryKey {
        attacker: StoneKey::from(attacker),
        position: CanonicalPosition::new(position).key(),
    }
}

fn ordered_legal_moves(position: &Position) -> Vec<Move> {
    if position.winner().is_some() || position.is_full() {
        return Vec::new();
    }
    let board = BoardState::new(position);
    let side = position.side_to_move();
    let center = BOARD_SIZE / 2;
    let candidates = board.candidate_bits();
    let mut moves: Vec<_> = Move::all().filter(|&at| position.is_legal(at)).collect();
    moves.sort_by_key(|&at| {
        (
            Reverse(board.patterns().profile(at, side)),
            Reverse(board.patterns().profile(at, side.opponent())),
            !candidates.test(at),
            at.row().abs_diff(center) + at.column().abs_diff(center),
            at.index(),
        )
    });
    moves
}

fn saturated_add(left: u64, right: u64) -> u64 {
    left.saturating_add(right).min(INFINITY)
}

pub(crate) fn verify_immediate(position: &Position, attacker: Stone) -> Option<u8> {
    if position.winner() == Some(attacker) {
        return Some(0);
    }
    if position.winner().is_some() || position.is_full() {
        return None;
    }
    let board = BoardState::new(position);
    match (
        position.side_to_move() == attacker,
        immediate_tactic(board.patterns(), position.side_to_move()),
    ) {
        (true, ImmediateTactic::Win(_)) => Some(1),
        (false, ImmediateTactic::Loss { .. }) => Some(2),
        _ => None,
    }
}

pub(crate) fn verify_tactical_line(
    position: &Position,
    attacker: Stone,
    is_vct: bool,
    max_plies: u8,
    max_nodes: u64,
) -> Option<(u8, Option<Move>)> {
    tactical_attempt(
        position,
        attacker,
        is_vct,
        ProofLimits::new(max_plies, max_nodes),
        max_nodes,
        None,
        CancellationToken::new(),
    )
    .0
}

fn tactical_attempt(
    position: &Position,
    attacker: Stone,
    is_vct: bool,
    limits: ProofLimits,
    outer_nodes: u64,
    duration: Option<Duration>,
    cancellation: CancellationToken,
) -> (Option<(u8, Option<Move>)>, u64) {
    let mut search_limits = SearchLimits::new(0).with_max_nodes(outer_nodes.min(limits.max_nodes));
    if let Some(duration) = duration {
        search_limits = search_limits.with_move_time(duration);
    }
    let mut budget = SearchBudget::new(search_limits, cancellation);
    let mut board = BoardState::new(position);
    let mut result = if is_vct {
        let mut solver = VctSolver::new(1);
        solver.begin_search(limits.max_nodes);
        let result = solver.solve_controlled(&mut board, attacker, limits.max_plies, &mut budget);
        match result.status {
            VctStatus::ProvenWin { plies } => {
                Some((plies, result.principal_variation.first().copied()))
            }
            VctStatus::NoProof | VctStatus::BudgetExceeded | VctStatus::Interrupted => None,
        }
    } else {
        let mut solver = VcfSolver::new();
        solver.begin_search(limits.max_nodes);
        let result = solver.solve_controlled(&mut board, attacker, limits.max_plies, &mut budget);
        match result.status {
            VcfStatus::ProvenWin { plies } => {
                Some((plies, result.principal_variation.first().copied()))
            }
            VcfStatus::NotProven | VcfStatus::BudgetExceeded | VcfStatus::Interrupted => None,
        }
    };
    if budget.poll().is_err() {
        result = None;
    }
    (result, budget.work_nodes())
}

const STAT_COUNT: usize = 13;

fn statistics_values(stats: SolverStatistics) -> [u64; STAT_COUNT] {
    [
        stats.work_nodes,
        stats.expanded_nodes,
        stats.generated_children,
        stats.exact_cache_hits,
        stats.vcf_attempts,
        stats.vcf_proven,
        stats.vct_attempts,
        stats.vct_proven,
        stats.progressive_widen_events,
        stats.resident_nodes as u64,
        stats.unresolved_nodes as u64,
        stats.root_proof_number,
        stats.root_disproof_number,
    ]
}

fn statistics_from_values(values: &[u64]) -> Result<SolverStatistics, SolverError> {
    if values.len() != STAT_COUNT {
        return Err(SolverError::Invalid("checkpoint statistics"));
    }
    Ok(SolverStatistics {
        work_nodes: values[0],
        expanded_nodes: values[1],
        generated_children: values[2],
        exact_cache_hits: values[3],
        vcf_attempts: values[4],
        vcf_proven: values[5],
        vct_attempts: values[6],
        vct_proven: values[7],
        progressive_widen_events: values[8],
        resident_nodes: usize::try_from(values[9])
            .map_err(|_| SolverError::Invalid("resident nodes"))?,
        unresolved_nodes: usize::try_from(values[10])
            .map_err(|_| SolverError::Invalid("unresolved nodes"))?,
        root_proof_number: values[11],
        root_disproof_number: values[12],
    })
}

fn stone_byte(stone: Stone) -> u8 {
    match stone {
        Stone::Black => 0,
        Stone::White => 1,
    }
}
fn decode_stone(value: u8) -> Result<Stone, SolverError> {
    match value {
        0 => Ok(Stone::Black),
        1 => Ok(Stone::White),
        _ => Err(SolverError::Invalid("stone tag")),
    }
}
fn move_byte(at: Move) -> u8 {
    u8::try_from(at.index()).unwrap_or(0)
}
fn decode_move(value: u8) -> Result<Move, SolverError> {
    Move::from_index(usize::from(value)).map_err(|_| SolverError::Invalid("move index"))
}
fn outcome_byte(outcome: ProofOutcome) -> u8 {
    match outcome {
        ProofOutcome::Unknown => 0,
        ProofOutcome::ProvenWin => 1,
        ProofOutcome::Refuted => 2,
    }
}
fn decode_outcome(value: u8) -> Result<ProofOutcome, SolverError> {
    match value {
        0 => Ok(ProofOutcome::Unknown),
        1 => Ok(ProofOutcome::ProvenWin),
        2 => Ok(ProofOutcome::Refuted),
        _ => Err(SolverError::Invalid("outcome tag")),
    }
}
fn decode_bool(value: u8) -> Result<bool, SolverError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(SolverError::Invalid("boolean tag")),
    }
}
fn write_u16(writer: &mut impl Write, value: usize) -> Result<(), SolverError> {
    let value = u16::try_from(value).map_err(|_| SolverError::Invalid("u16 overflow"))?;
    writer
        .write_all(&value.to_le_bytes())
        .map_err(SolverError::Io)
}
fn write_optional_index(writer: &mut impl Write, value: Option<usize>) -> Result<(), SolverError> {
    match value {
        None => writer.write_all(&[0]).map_err(SolverError::Io),
        Some(value) => {
            writer.write_all(&[1]).map_err(SolverError::Io)?;
            let value = u32::try_from(value).map_err(|_| SolverError::Invalid("node index"))?;
            writer
                .write_all(&value.to_le_bytes())
                .map_err(SolverError::Io)
        }
    }
}
fn write_optional_move(writer: &mut impl Write, value: Option<Move>) -> Result<(), SolverError> {
    match value {
        None => writer.write_all(&[0]).map_err(SolverError::Io),
        Some(at) => writer
            .write_all(&[1, move_byte(at)])
            .map_err(SolverError::Io),
    }
}
fn write_evidence(writer: &mut impl Write, evidence: Evidence) -> Result<(), SolverError> {
    let (tag, extra): (u8, Option<(u8, u64)>) = match evidence {
        Evidence::Unknown => (0, None),
        Evidence::Terminal => (1, None),
        Evidence::Immediate => (2, None),
        Evidence::Vcf {
            max_plies,
            max_nodes,
        } => (3, Some((max_plies, max_nodes))),
        Evidence::Vct {
            max_plies,
            max_nodes,
        } => (4, Some((max_plies, max_nodes))),
        Evidence::Aggregated => (5, None),
        Evidence::Cached(source) => {
            writer.write_all(&[6]).map_err(SolverError::Io)?;
            let source = u32::try_from(source).map_err(|_| SolverError::Invalid("cache source"))?;
            return writer
                .write_all(&source.to_le_bytes())
                .map_err(SolverError::Io);
        }
    };
    writer.write_all(&[tag]).map_err(SolverError::Io)?;
    if let Some((plies, nodes)) = extra {
        writer.write_all(&[plies]).map_err(SolverError::Io)?;
        writer
            .write_all(&nodes.to_le_bytes())
            .map_err(SolverError::Io)?;
    }
    Ok(())
}

struct CheckpointDecoder<'a, R> {
    reader: &'a mut R,
}
impl<R: Read> CheckpointDecoder<'_, R> {
    fn exact(&mut self, bytes: &mut [u8]) -> Result<(), SolverError> {
        self.reader.read_exact(bytes).map_err(SolverError::Io)
    }
    fn u8(&mut self) -> Result<u8, SolverError> {
        let mut b = [0];
        self.exact(&mut b)?;
        Ok(b[0])
    }
    fn u16(&mut self) -> Result<u16, SolverError> {
        let mut b = [0; 2];
        self.exact(&mut b)?;
        Ok(u16::from_le_bytes(b))
    }
    fn u32(&mut self) -> Result<u32, SolverError> {
        let mut b = [0; 4];
        self.exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn u64(&mut self) -> Result<u64, SolverError> {
        let mut b = [0; 8];
        self.exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn key(&mut self) -> Result<CanonicalPositionKey, SolverError> {
        let mut b = [0; CanonicalPositionKey::BYTE_LEN];
        self.exact(&mut b)?;
        CanonicalPositionKey::from_bytes(b).map_err(|_| SolverError::Invalid("canonical key"))
    }
    fn optional_index(&mut self) -> Result<Option<usize>, SolverError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(
                usize::try_from(self.u32()?).map_err(|_| SolverError::Invalid("node index"))?,
            )),
            _ => Err(SolverError::Invalid("optional index tag")),
        }
    }
    fn optional_move(&mut self) -> Result<Option<Move>, SolverError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(decode_move(self.u8()?)?)),
            _ => Err(SolverError::Invalid("optional move tag")),
        }
    }
    fn evidence(&mut self) -> Result<Evidence, SolverError> {
        Ok(match self.u8()? {
            0 => Evidence::Unknown,
            1 => Evidence::Terminal,
            2 => Evidence::Immediate,
            3 => Evidence::Vcf {
                max_plies: self.u8()?,
                max_nodes: self.u64()?,
            },
            4 => Evidence::Vct {
                max_plies: self.u8()?,
                max_nodes: self.u64()?,
            },
            5 => Evidence::Aggregated,
            6 => Evidence::Cached(
                usize::try_from(self.u32()?).map_err(|_| SolverError::Invalid("cache source"))?,
            ),
            _ => return Err(SolverError::Invalid("evidence tag")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AlphaBetaEngine, PatternEvaluator, ProofSource, SearchEngine};
    use rustmoku_core::Symmetry;
    use std::sync::Arc;

    fn fixture() -> Game {
        let mut game = Game::new(RuleSet::Freestyle);
        // Black has an immediate horizontal winning move at internal index 4.
        for index in [0, 15, 1, 16, 2, 17, 3, 30] {
            game.play_move(Move::from_index(index).unwrap()).unwrap();
        }
        game
    }

    fn vcf_fixture() -> Game {
        let mut game = Game::new(RuleSet::Freestyle);
        for index in [108, 107, 109, 0, 110, 2, 66, 4, 81, 6] {
            game.play_move(Move::from_index(index).unwrap()).unwrap();
        }
        game
    }

    #[test]
    fn tiny_solve_book_verify_and_query() {
        let game = fixture();
        let mut solver = OfflineSolver::new(&game, Stone::Black).unwrap();
        let result = solver.solve(SolverLimits::new(10));
        assert_eq!(result.outcome, ProofOutcome::ProvenWin);
        let book = solver.export_proof_book().unwrap();
        let mut bytes = Vec::new();
        book.write_to(&mut bytes).unwrap();
        let verified = ProofBook::read_from(&mut bytes.as_slice())
            .unwrap()
            .verify()
            .unwrap();
        let hit = verified.query(game.position()).unwrap();
        assert_eq!(hit.best_move, Move::from_index(4).unwrap());
        assert_eq!(hit.distance, ProofDistance::AtMost(1));
    }

    #[test]
    fn zero_budget_is_unknown_and_not_exportable() {
        let game = Game::new(RuleSet::Freestyle);
        let mut solver = OfflineSolver::new(&game, Stone::Black).unwrap();
        assert_eq!(
            solver.solve(SolverLimits::new(0)).outcome,
            ProofOutcome::Unknown
        );
        assert!(matches!(
            solver.export_proof_book(),
            Err(SolverError::Incomplete)
        ));
    }

    #[test]
    fn partial_defender_expansion_keeps_a_virtual_unknown() {
        let mut game = Game::new(RuleSet::Freestyle);
        game.play_move(Move::CENTER).unwrap();
        let mut solver = OfflineSolver::new(&game, Stone::Black).unwrap();
        let result = solver.solve(
            SolverLimits::new(5)
                .with_vcf(ProofLimits::new(0, 0))
                .with_vct(ProofLimits::new(0, 0)),
        );
        assert_eq!(result.outcome, ProofOutcome::Unknown);
        assert!(result.statistics.root_proof_number > 0);
        assert!(solver.nodes[0].next_unexpanded < solver.nodes[0].ordered_moves.len());
    }

    #[test]
    fn checkpoint_resume_matches_the_same_work_sequence() {
        let game = Game::new(RuleSet::Freestyle);
        let limits = SolverLimits::new(10)
            .with_vcf(ProofLimits::new(0, 0))
            .with_vct(ProofLimits::new(0, 0));
        let mut uninterrupted = OfflineSolver::new(&game, Stone::Black).unwrap();
        let mut saved = OfflineSolver::new(&game, Stone::Black).unwrap();
        assert_eq!(uninterrupted.solve(limits), saved.solve(limits));
        let path =
            std::env::temp_dir().join(format!("rustmoku-checkpoint-{}.bin", std::process::id()));
        saved.save_checkpoint(&path).unwrap();
        let mut resumed = OfflineSolver::load_checkpoint(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(uninterrupted.solve(limits), resumed.solve(limits));
    }

    #[test]
    fn verified_query_and_runtime_hit_follow_d4_orientation() {
        let game = fixture();
        let mut solver = OfflineSolver::new(&game, Stone::Black).unwrap();
        solver.solve(SolverLimits::new(10));
        let verified = Arc::new(solver.export_proof_book().unwrap().verify().unwrap());
        for symmetry in [
            Symmetry::Identity,
            Symmetry::Rotate90,
            Symmetry::MirrorMainDiagonal,
        ] {
            let mut transformed = Game::new(RuleSet::Freestyle);
            for at in game.history() {
                transformed.play_move(symmetry.transform(at)).unwrap();
            }
            let expected = symmetry.transform(Move::from_index(4).unwrap());
            assert_eq!(
                verified.query(transformed.position()).unwrap().best_move,
                expected
            );
        }
        let mut immediate_engine =
            AlphaBetaEngine::new(PatternEvaluator).with_proof_book(Arc::clone(&verified));
        let immediate = immediate_engine.search(game.position(), SearchLimits::new(1));
        assert_eq!(immediate.best_move, Some(Move::from_index(4).unwrap()));
        assert_eq!(immediate.statistics.proof_book_probes, 0);
        let tactical_game = vcf_fixture();
        let mut tactical_solver = OfflineSolver::new(&tactical_game, Stone::Black).unwrap();
        let tactical_limits = SolverLimits::new(10_000)
            .with_vcf(ProofLimits::new(7, 5_000))
            .with_vct(ProofLimits::new(0, 0));
        assert_eq!(
            tactical_solver.solve(tactical_limits).outcome,
            ProofOutcome::ProvenWin
        );
        let tactical_book = Arc::new(
            tactical_solver
                .export_proof_book()
                .unwrap()
                .verify()
                .unwrap(),
        );
        let expected = tactical_book
            .query(tactical_game.position())
            .unwrap()
            .best_move;
        let mut engine = AlphaBetaEngine::new(PatternEvaluator).with_proof_book(tactical_book);
        let result = engine.search(tactical_game.position(), SearchLimits::new(1));
        assert_eq!(result.best_move, Some(expected));
        assert_eq!(result.proof.unwrap().source, ProofSource::ProofBook);
        assert_eq!(
            (
                result.statistics.proof_book_probes,
                result.statistics.proof_book_hits
            ),
            (1, 1)
        );
        let zero = engine.search(tactical_game.position(), SearchLimits::new(0));
        assert_eq!(
            (
                zero.statistics.proof_book_probes,
                zero.statistics.proof_book_hits
            ),
            (0, 0)
        );
        let quiet = Game::new(RuleSet::Freestyle);
        let miss = engine.search(quiet.position(), SearchLimits::new(1));
        let baseline =
            AlphaBetaEngine::new(PatternEvaluator).search(quiet.position(), SearchLimits::new(1));
        assert_eq!(
            (
                miss.best_move,
                miss.score,
                miss.completed_depth,
                miss.principal_variation
            ),
            (
                baseline.best_move,
                baseline.score,
                baseline.completed_depth,
                baseline.principal_variation
            )
        );
        assert_eq!(
            (
                miss.statistics.proof_book_probes,
                miss.statistics.proof_book_hits
            ),
            (1, 0)
        );
    }

    #[test]
    fn truncated_book_never_verifies() {
        let game = fixture();
        let mut solver = OfflineSolver::new(&game, Stone::Black).unwrap();
        solver.solve(SolverLimits::new(10));
        let book = solver.export_proof_book().unwrap();
        let mut bytes = Vec::new();
        book.write_to(&mut bytes).unwrap();
        bytes.pop();
        assert!(ProofBook::read_from(&mut bytes.as_slice()).is_err());
    }
}
