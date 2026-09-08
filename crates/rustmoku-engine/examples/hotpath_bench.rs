use std::{env, error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let mut iterations = 100_000;
    let mut model = None;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--iterations" => iterations = args.next().ok_or("missing iterations")?.parse()?,
            "--model" => model = Some(PathBuf::from(args.next().ok_or("missing model")?)),
            _ if argument.bytes().all(|byte| byte.is_ascii_digit()) => {
                iterations = argument.parse()?;
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    if iterations == 0 {
        return Err("iterations must be positive".into());
    }
    if let Some(model) = model {
        rustmoku_engine::benchmarks::run_learned_hotpath(iterations, model)?;
    } else {
        rustmoku_engine::benchmarks::run_hotpath(iterations);
    }
    Ok(())
}
