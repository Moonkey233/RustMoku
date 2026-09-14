//! Bounded offline replay protocol. No learned/search score is an exact fact.
use rustmoku_core::{CanonicalPosition, Game, Move, RuleSet, Stone};
use std::{
    error::Error,
    fmt::Write as _,
    io::{self, BufRead, Read, Write},
};

pub fn moves_hex(game: &Game) -> String {
    game.history()
        .map(|at| format!("{:02x}", at.index()))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn replay(encoded: &str) -> Result<Game, Box<dyn Error>> {
    if encoded.len() > 450 || !encoded.len().is_multiple_of(2) || !encoded.is_ascii() {
        return Err("invalid bounded move sequence".into());
    }
    let mut game = Game::new(RuleSet::Freestyle);
    for offset in (0..encoded.len()).step_by(2) {
        game.play_move(Move::from_index(usize::from(u8::from_str_radix(
            &encoded[offset..offset + 2],
            16,
        )?))?)?;
    }
    Ok(game)
}

fn response(encoded: &str) -> Result<String, Box<dyn Error>> {
    let game = replay(encoded)?;
    let position = game.position();
    let canonical = CanonicalPosition::new(position);
    let stone = |s| if s == Stone::Black { 0 } else { 1 };
    let winner = position
        .winner()
        .map_or_else(|| "null".to_owned(), |s| stone(s).to_string());
    let mut output = format!(
        "{{\"version\":1,\"key\":\"{}\",\"side\":{},\"winner\":{winner},\"full\":{},\"children\":[",
        hex(canonical.key().as_bytes()),
        stone(position.side_to_move()),
        position.is_full()
    );
    // One mutable root copy, bounded make/unmake; all legal replies, no pruning.
    let mut working = position.clone();
    let mut first = true;
    for at in Move::all().filter(|&at| position.is_legal(at)) {
        let undo = working.make_move(at)?;
        let child = CanonicalPosition::new(&working);
        if !first {
            output.push(',');
        }
        first = false;
        let child_winner = working
            .winner()
            .map_or_else(|| "null".to_owned(), |s| stone(s).to_string());
        write!(
            output,
            "[{}, {}, \"{}\", {child_winner}, {}]",
            at.index(),
            canonical.move_to_canonical(at).index(),
            hex(child.key().as_bytes()),
            working.is_full()
        )?;
        working.unmake_move(undo);
    }
    output.push_str("]}");
    Ok(output)
}

pub fn worker() -> Result<(), Box<dyn Error>> {
    let input = io::stdin();
    let mut input = input.lock();
    let output = io::stdout();
    let mut output = output.lock();
    loop {
        let mut line = String::new();
        let count = input.by_ref().take(453).read_line(&mut line)?;
        if count == 0 {
            return Ok(());
        }
        if count > 452 || !line.ends_with('\n') {
            return Err("oversized or truncated facts request".into());
        }
        writeln!(output, "{}", response(line.trim_end_matches(['\r', '\n']))?)?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_rejects_duplicates_and_oversized_input() {
        assert!(replay("0000").is_err());
        assert!(replay(&"00".repeat(226)).is_err());
        assert!(response("70").unwrap().contains("\"side\":1"));
    }
}
