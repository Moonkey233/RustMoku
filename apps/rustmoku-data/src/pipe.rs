//! Concrete pipe endpoint for independent RustMoku process comparisons.
use rustmoku_core::{Game, Move, Stone};
use rustmoku_engine::{AlphaBetaEngine, RuntimeEvaluator, SearchEngine, SearchLimits};
use std::io::{BufRead, Read, Write};
use std::time::Duration;

fn coordinates(text: &str) -> Result<Move, String> {
    let (column, row) = text.split_once(',').ok_or("expected column,row")?;
    Move::from_row_col(
        row.parse::<usize>().map_err(|_| "invalid row")?,
        column.parse::<usize>().map_err(|_| "invalid column")?,
    )
    .map_err(|error| error.to_string())
}

pub(super) fn run(
    mut engine: AlphaBetaEngine<RuntimeEvaluator>,
    depth: u8,
    nodes: u64,
    mut input: impl BufRead,
    mut output: impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut game = Game::default();
    let mut own = None;
    let mut board = None::<Vec<(Move, u8)>>;
    let mut turn_ms = 1000_u64;
    let mut left_ms = u64::MAX;
    loop {
        let mut line = String::new();
        if (&mut input).take(4097).read_line(&mut line)? == 0 {
            break;
        }
        if line.len() > 4096 {
            return Err("protocol line too long".into());
        }
        let line = line.trim();
        let choose = if let Some(entries) = board.as_mut() {
            if line == "DONE" {
                let entries = board.take().unwrap();
                let own_stone = if entries.len().is_multiple_of(2) {
                    Stone::Black
                } else {
                    Stone::White
                };
                game = Game::default();
                for (at, role) in entries {
                    let expected = if game.position().side_to_move() == own_stone {
                        1
                    } else {
                        2
                    };
                    if role != expected {
                        return Err("BOARD requires legal chronological history".into());
                    }
                    game.play_move(at)?;
                }
                own = Some(own_stone);
                true
            } else {
                if entries.len() == 225 {
                    return Err("BOARD too long".into());
                }
                let (point, role) = line.rsplit_once(',').ok_or("invalid BOARD row")?;
                entries.push((coordinates(point)?, role.parse()?));
                false
            }
        } else if line == "START 15" || line == "RESTART" {
            game = Game::default();
            own = None;
            engine.clear_transposition_table();
            writeln!(output, "OK")?;
            output.flush()?;
            false
        } else if line == "END" {
            return Ok(());
        } else if line == "BOARD" {
            board = Some(Vec::new());
            false
        } else if line == "BEGIN" {
            if game.position().move_count() != 0 {
                return Err("BEGIN requires empty game".into());
            }
            own = Some(Stone::Black);
            true
        } else if let Some(point) = line.strip_prefix("TURN ") {
            game.play_move(coordinates(point)?)?;
            true
        } else if let Some(value) = line.strip_prefix("INFO timeout_turn ") {
            turn_ms = value.parse()?;
            false
        } else if let Some(value) = line.strip_prefix("INFO time_left ") {
            left_ms = value.parse()?;
            false
        } else if line == "INFO rule 0" || line.starts_with("INFO timeout_match ") {
            false
        } else {
            return Err(format!("unsupported protocol command: {line}").into());
        };
        if choose {
            if own != Some(game.position().side_to_move()) {
                return Err("protocol side mismatch".into());
            }
            let hard = Duration::from_millis(turn_ms.min(left_ms));
            let result = engine.search(
                game.position(),
                SearchLimits::new(depth)
                    .with_max_nodes(nodes)
                    .with_move_time(hard.saturating_sub(hard / 10)),
            );
            let at = result
                .best_move
                .ok_or("no move for ongoing protocol game")?;
            game.play_move(at)?;
            writeln!(output, "{},{}", at.column(), at.row())?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustmoku_engine::EngineConfig;

    #[test]
    fn chronological_pipe_replays_both_colors_and_rejects_invalid_history() {
        for commands in [
            "START 15\nBEGIN\nEND\n",
            "START 15\nBOARD\n7,7,2\nDONE\nEND\n",
            "START 15\nBOARD\n7,7,1\n8,7,2\nDONE\nEND\n",
        ] {
            let engine =
                AlphaBetaEngine::with_config(RuntimeEvaluator::Pattern, EngineConfig::new(1));
            let mut output = Vec::new();
            run(engine, 1, 64, commands.as_bytes(), &mut output).unwrap();
            let text = String::from_utf8(output).unwrap();
            assert_eq!(text.lines().count(), 2);
            assert_eq!(text.lines().next(), Some("OK"));
            assert!(coordinates(text.lines().nth(1).unwrap()).is_ok());
        }
        for commands in [
            "START 15\nBOARD\n7,7,1\nDONE\n",
            "START 15\nBOARD\n7,7,1\n7,7,2\nDONE\n",
        ] {
            let engine =
                AlphaBetaEngine::with_config(RuntimeEvaluator::Pattern, EngineConfig::new(1));
            assert!(run(engine, 1, 64, commands.as_bytes(), Vec::new()).is_err());
        }
    }
}
