//! Small versioned move-sequence records. Import always replays through Game.
use crate::{Game, Move, MoveError, RuleSet};
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum RecordError {
    Syntax {
        line: usize,
        expected: &'static str,
    },
    UnsupportedVersion(String),
    UnsupportedRules(String),
    InvalidCoordinate {
        ply: usize,
        text: String,
    },
    IllegalMove {
        ply: usize,
        at: Move,
        source: MoveError,
    },
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { line, expected } => write!(f, "record line {line}: expected {expected}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported RustMoku record version: {version}")
            }
            Self::UnsupportedRules(rules) => write!(f, "unsupported record rules: {rules}"),
            Self::InvalidCoordinate { ply, text } => write!(
                f,
                "move {ply}: invalid coordinate {text:?}; expected A1..O15"
            ),
            Self::IllegalMove { ply, at, source } => write!(f, "move {ply} ({at}): {source}"),
        }
    }
}
impl std::error::Error for RecordError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IllegalMove { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl Game {
    /// Deterministic UTF-8 text with uppercase coordinates and a final newline.
    #[must_use]
    pub fn to_record(&self) -> String {
        let rules = match self.position().rules() {
            RuleSet::Freestyle => "freestyle",
        };
        let mut text = format!("RustMoku 1\nrules={rules}\nmoves=");
        for (index, at) in self.history().enumerate() {
            if index != 0 {
                text.push(' ');
            }
            text.push_str(&at.to_string());
        }
        text.push('\n');
        text
    }

    /// Build a fresh game via legal replay; errors never mutate an existing game.
    /// Accepts LF/CRLF, whitespace-separated moves and lowercase coordinates.
    pub fn from_record(text: &str) -> Result<Self, RecordError> {
        let mut lines = text.lines();
        let version = lines
            .next()
            .and_then(|line| line.strip_prefix("RustMoku "))
            .ok_or(RecordError::Syntax {
                line: 1,
                expected: "RustMoku 1",
            })?;
        if version != "1" {
            return Err(RecordError::UnsupportedVersion(version.into()));
        }
        let rules = lines
            .next()
            .and_then(|line| line.strip_prefix("rules="))
            .ok_or(RecordError::Syntax {
                line: 2,
                expected: "rules=freestyle",
            })?;
        let rules = match rules {
            "freestyle" => RuleSet::Freestyle,
            other => return Err(RecordError::UnsupportedRules(other.into())),
        };
        let moves = lines
            .next()
            .and_then(|line| line.strip_prefix("moves="))
            .ok_or(RecordError::Syntax {
                line: 3,
                expected: "moves=...",
            })?;
        if lines.next().is_some() {
            return Err(RecordError::Syntax {
                line: 4,
                expected: "end of record",
            });
        }
        let mut game = Self::new(rules);
        for (index, token) in moves.split_whitespace().enumerate() {
            let ply = index + 1;
            let at = token.parse().map_err(|_| RecordError::InvalidCoordinate {
                ply,
                text: token.into(),
            })?;
            game.play_move(at)
                .map_err(|source| RecordError::IllegalMove { ply, at, source })?;
        }
        Ok(game)
    }
}
