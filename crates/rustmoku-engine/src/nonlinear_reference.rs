//! Offline architecture-2 float oracle. Never selected by production search.
//!
//! Four directional embeddings plus an explicit center embedding are aggregated
//! before ReLU. This adds cross-direction interaction that cannot be folded into
//! independent scalar line weights. Value averages all activated centers.

use crate::{PatternState, pattern::LineKey};
use rustmoku_core::{CELL_COUNT, Move, Position, Stone};
use std::{io::Read, path::Path};

const WIDTH: usize = 8;
const FEATURES: usize = 1 << 16;
const VALUES: usize = FEATURES * WIDTH + 3 * WIDTH + WIDTH * 2 + 1;

pub struct NonlinearReference {
    embedding: Box<[[f64; WIDTH]]>,
    center: [[f64; WIDTH]; 3],
    value: [f64; WIDTH],
    policy: [f64; WIDTH],
    bias: f64,
}

impl NonlinearReference {
    pub fn read(path: impl AsRef<Path>) -> Result<Self, String> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|error| error.to_string())?
            .take((16 + VALUES * 8 + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() != 16 + VALUES * 8
            || &bytes[..8] != b"RMLREF02"
            || bytes[8..16] != [2, 0, 8, 0, 0, 0, 1, 0]
        {
            return Err("invalid architecture-2 reference format".into());
        }
        let mut values = Vec::with_capacity(VALUES);
        for bytes in bytes[16..].as_chunks::<8>().0 {
            let value = f64::from_le_bytes(*bytes);
            if !value.is_finite() || value.abs() > 16.0 {
                return Err("reference weight must be finite and within +/-16".into());
            }
            values.push(value);
        }
        let embedding = values[..FEATURES * WIDTH]
            .as_chunks::<WIDTH>()
            .0
            .to_vec()
            .into_boxed_slice();
        let tail = &values[FEATURES * WIDTH..];
        Ok(Self {
            embedding,
            center: std::array::from_fn(|i| tail[i * WIDTH..(i + 1) * WIDTH].try_into().unwrap()),
            value: tail[3 * WIDTH..4 * WIDTH].try_into().unwrap(),
            policy: tail[4 * WIDTH..5 * WIDTH].try_into().unwrap(),
            bias: tail[5 * WIDTH],
        })
    }

    fn activated(&self, keys: [LineKey; 4], cell: Option<Stone>, side: Stone) -> [f64; WIDTH] {
        let center = cell.map_or(0, |stone| if stone == side { 1 } else { 2 });
        let mut value = self.center[center];
        for key in keys {
            let key = relative(key, side);
            for (sum, weight) in value.iter_mut().zip(self.embedding[usize::from(key.0)]) {
                *sum += weight;
            }
        }
        value.map(|value| value.max(0.0))
    }

    fn value_from(
        &self,
        side: Stone,
        mut features: impl FnMut(Move) -> ([LineKey; 4], Option<Stone>),
    ) -> f64 {
        let mut total = self.bias;
        for at in Move::all() {
            let (keys, cell) = features(at);
            let local = self.activated(keys, cell, side);
            total += local
                .iter()
                .zip(self.value)
                .map(|(a, b)| a * b)
                .sum::<f64>()
                / CELL_COUNT as f64;
        }
        total
    }

    pub fn value(&self, position: &Position) -> f64 {
        let patterns = PatternState::new(position);
        self.value_from(position.side_to_move(), |at| {
            (patterns.line_keys(at), position.cell(at))
        })
    }

    pub fn policy(&self, position: &Position, at: Move) -> Option<f64> {
        let patterns = PatternState::new(position);
        position.is_legal(at).then(|| {
            self.activated(patterns.line_keys(at), None, position.side_to_move())
                .iter()
                .zip(self.policy)
                .map(|(a, b)| a * b)
                .sum()
        })
    }
}

fn relative(key: LineKey, side: Stone) -> LineKey {
    crate::learned::relative_key(key, side)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_geometry::LINE_INFLUENCES;

    #[test]
    fn nonlinear_reference_features_restore_and_match_rebuild() {
        let model = NonlinearReference {
            embedding: (0..FEATURES)
                .map(|key| std::array::from_fn(|d| ((key + d * 7) % 31) as f64 / 32.0 - 0.5))
                .collect(),
            center: [[0.0; WIDTH], [0.25; WIDTH], [-0.25; WIDTH]],
            value: [0.125; WIDTH],
            policy: [-0.125; WIDTH],
            bias: 0.25,
        };
        let mut position = Position::default();
        let mut patterns = PatternState::new(&position);
        let mut keys = std::array::from_fn::<_, CELL_COUNT, _>(|i| {
            patterns.line_keys(Move::from_index(i).unwrap())
        });
        let mut cells = [None; CELL_COUNT];
        let mut undos = Vec::new();
        for index in (0..CELL_COUNT).map(|i| (i * 97) % CELL_COUNT).take(100) {
            let at = Move::from_index(index).unwrap();
            if !position.is_legal(at) {
                break;
            }
            let stone = position.side_to_move();
            let board_undo = position.make_move(at).unwrap();
            let pattern_undo = patterns.make_move(at, stone);
            let delta = pattern_undo.delta();
            for (influence, (_, new)) in LINE_INFLUENCES[at.index()].iter().zip(delta.changes()) {
                keys[influence.center.index()][usize::from(influence.direction)] = new;
            }
            cells[at.index()] = Some(stone);
            let incremental = model.value_from(position.side_to_move(), |at| {
                (keys[at.index()], cells[at.index()])
            });
            assert!((incremental - model.value(&position)).abs() < 1e-12);
            undos.push((at, board_undo, pattern_undo));
        }
        while let Some((at, board_undo, pattern_undo)) = undos.pop() {
            let delta = pattern_undo.delta();
            for (influence, (old, _)) in LINE_INFLUENCES[at.index()].iter().zip(delta.changes()) {
                keys[influence.center.index()][usize::from(influence.direction)] = old;
            }
            cells[at.index()] = None;
            patterns.unmake_move(pattern_undo);
            position.unmake_move(board_undo);
            let incremental = model.value_from(position.side_to_move(), |at| {
                (keys[at.index()], cells[at.index()])
            });
            assert!((incremental - model.value(&position)).abs() < 1e-12);
        }
        assert_eq!(position, Position::default());
    }

    #[test]
    fn relative_color_codes_keep_empty_and_boundaries() {
        for key in 0..=u16::MAX {
            let mut reference = 0;
            for field in 0..8 {
                let code = (key >> (field * 2)) & 3;
                reference |= match code {
                    1 => 2,
                    2 => 1,
                    value => value,
                } << (field * 2);
            }
            assert_eq!(relative(LineKey(key), Stone::White), LineKey(reference));
        }
    }
}
