use std::{env, error::Error};

fn main() -> Result<(), Box<dyn Error>> {
    let iterations = env::args()
        .nth(1)
        .map_or(Ok(100_000), |value| value.parse::<usize>())?;
    if iterations == 0 {
        return Err("iterations must be positive".into());
    }
    rustmoku_engine::benchmarks::run_hotpath(iterations);
    Ok(())
}
