use rustmoku_core::{Game, Move};
use rustmoku_engine::nonlinear_reference::NonlinearReference;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: nonlinear_reference MODEL RECORD MOVE".into());
    }
    let model = NonlinearReference::read(&args[0])?;
    let game = Game::from_record(&std::fs::read_to_string(&args[1])?)?;
    let at: Move = args[2].parse()?;
    println!("value={:.17}", model.value(game.position()));
    println!(
        "policy={:.17}",
        model
            .policy(game.position(), at)
            .ok_or("illegal policy move")?
    );
    Ok(())
}
