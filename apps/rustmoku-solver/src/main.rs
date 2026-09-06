#![forbid(unsafe_code)]

use std::{env, error::Error, fs, path::PathBuf, process::ExitCode, time::Duration};

use rustmoku_core::{Game, Stone};
use rustmoku_engine::{
    OfflineSolver, ProofBook, ProofLimits, ProofOutcome, SolverLimits, SolverResult,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rustmoku-solver: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = Arguments::new(env::args().skip(1));
    match args.command()?.as_str() {
        "solve" => solve(args),
        "resume" => resume(args),
        "verify" => verify(args),
        "inspect" => inspect(args),
        "query" => query(args),
        "help" | "--help" | "-h" => {
            usage();
            Ok(())
        }
        other => Err(format!("unknown command {other:?}").into()),
    }
}

fn solve(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let record = read_game(&args.required("--record")?)?;
    let attacker = parse_stone(&args.required("--attacker")?)?;
    let checkpoint = PathBuf::from(args.required("--checkpoint")?);
    let output = PathBuf::from(args.required("--output")?);
    let limits = args.limits()?;
    args.finish()?;
    let mut solver = OfflineSolver::new(&record, attacker)?;
    finish_solve(&mut solver, limits, &checkpoint, &output)
}

fn resume(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let checkpoint = PathBuf::from(args.required("--checkpoint")?);
    let output = PathBuf::from(args.required("--output")?);
    let limits = args.limits()?;
    args.finish()?;
    let mut solver = OfflineSolver::load_checkpoint(&checkpoint)?;
    finish_solve(&mut solver, limits, &checkpoint, &output)
}

fn finish_solve(
    solver: &mut OfflineSolver,
    limits: SolverLimits,
    checkpoint: &PathBuf,
    output: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let result = solver.solve(limits);
    print_result(result);
    match result.outcome {
        ProofOutcome::ProvenWin => {
            let book = solver.export_proof_book()?;
            book.write_to_path(output)?;
            // Parse and verify a fresh instance; generator state is not trusted.
            let verified = ProofBook::read_from_path(output)?.verify()?;
            let metadata = verified.metadata();
            println!(
                "verified book: roots={} entries={} path={}",
                metadata.roots,
                metadata.entries,
                output.display()
            );
        }
        ProofOutcome::Unknown => {
            solver.save_checkpoint(checkpoint)?;
            println!("incomplete checkpoint: {}", checkpoint.display());
        }
        ProofOutcome::Refuted => println!("exact refutation; no Proof Book exported"),
    }
    Ok(())
}

fn verify(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(args.required("--book")?);
    args.finish()?;
    let verified = ProofBook::read_from_path(&path)?.verify()?;
    let metadata = verified.metadata();
    println!(
        "verified: version={} rules={:?} roots={} entries={}",
        metadata.version, metadata.rules, metadata.roots, metadata.entries
    );
    Ok(())
}

fn inspect(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(args.required("--book")?);
    args.finish()?;
    let book = ProofBook::read_from_path(&path)?;
    let metadata = book.metadata();
    println!("version: {}", metadata.version);
    println!("rules: {:?}", metadata.rules);
    println!("roots: {}", metadata.roots);
    println!("entries: {}", metadata.entries);
    println!(
        "attackers: black_roots={} white_roots={}",
        metadata.black_roots, metadata.white_roots
    );
    println!(
        "sources: moves={} defender_all={} immediate={} vcf={} vct={}",
        metadata.sources.attacker_moves,
        metadata.sources.defender_nodes,
        metadata.sources.immediate_leaves,
        metadata.sources.vcf_leaves,
        metadata.sources.vct_leaves
    );
    println!("bytes: {}", fs::metadata(path)?.len());
    println!("trust: unverified (run verify for full strategy validation)");
    Ok(())
}

fn query(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let book = PathBuf::from(args.required("--book")?);
    let game = read_game(&args.required("--record")?)?;
    args.finish()?;
    let verified = ProofBook::read_from_path(book)?.verify()?;
    if let Some(hit) = verified.query(game.position()) {
        println!("hit: move={} distance={:?}", hit.best_move, hit.distance);
    } else {
        println!("miss");
    }
    Ok(())
}

fn read_game(path: &str) -> Result<Game, Box<dyn Error>> {
    Ok(Game::from_record(&fs::read_to_string(path)?)?)
}

fn parse_stone(value: &str) -> Result<Stone, Box<dyn Error>> {
    match value.to_ascii_lowercase().as_str() {
        "black" | "b" => Ok(Stone::Black),
        "white" | "w" => Ok(Stone::White),
        _ => Err("attacker must be black or white".into()),
    }
}

fn print_result(result: SolverResult) {
    let stats = result.statistics;
    println!("outcome: {:?}", result.outcome);
    println!("termination: {:?}", result.termination);
    println!(
        "work={} expanded={} generated={} cache_hits={} resident={} unresolved={} pn={} dn={}",
        stats.work_nodes,
        stats.expanded_nodes,
        stats.generated_children,
        stats.exact_cache_hits,
        stats.resident_nodes,
        stats.unresolved_nodes,
        stats.root_proof_number,
        stats.root_disproof_number
    );
    println!(
        "vcf={}/{} vct={}/{} widening={}",
        stats.vcf_proven,
        stats.vcf_attempts,
        stats.vct_proven,
        stats.vct_attempts,
        stats.progressive_widen_events
    );
}

struct Arguments {
    values: Vec<String>,
}

impl Arguments {
    fn new(values: impl Iterator<Item = String>) -> Self {
        Self {
            values: values.collect(),
        }
    }

    fn command(&mut self) -> Result<String, Box<dyn Error>> {
        if self.values.is_empty() {
            usage();
            return Err("a command is required".into());
        }
        Ok(self.values.remove(0))
    }

    fn take(&mut self, name: &str) -> Result<Option<String>, Box<dyn Error>> {
        let Some(index) = self.values.iter().position(|value| value == name) else {
            return Ok(None);
        };
        if index + 1 >= self.values.len() {
            return Err(format!("{name} requires a value").into());
        }
        self.values.remove(index);
        Ok(Some(self.values.remove(index)))
    }

    fn required(&mut self, name: &str) -> Result<String, Box<dyn Error>> {
        self.take(name)?
            .ok_or_else(|| format!("missing required {name}").into())
    }

    fn limits(&mut self) -> Result<SolverLimits, Box<dyn Error>> {
        let nodes = self.required("--nodes")?.parse()?;
        let mut limits = SolverLimits::new(nodes);
        if let Some(seconds) = self.take("--seconds")? {
            limits = limits.with_duration(Duration::from_secs(seconds.parse()?));
        }
        if let Some(maximum) = self.take("--resident-nodes")? {
            limits = limits.with_max_resident_nodes(maximum.parse()?);
        }
        let vcf_plies = self
            .take("--vcf-plies")?
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or(limits.vcf.max_plies);
        let vcf_nodes = self
            .take("--vcf-nodes")?
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or(limits.vcf.max_nodes);
        let vct_plies = self
            .take("--vct-plies")?
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or(limits.vct.max_plies);
        let vct_nodes = self
            .take("--vct-nodes")?
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or(limits.vct.max_nodes);
        limits = limits
            .with_vcf(ProofLimits::new(vcf_plies, vcf_nodes))
            .with_vct(ProofLimits::new(vct_plies, vct_nodes));
        Ok(limits)
    }

    fn finish(self) -> Result<(), Box<dyn Error>> {
        if self.values.is_empty() {
            Ok(())
        } else {
            Err(format!("unexpected arguments: {}", self.values.join(" ")).into())
        }
    }
}

fn usage() {
    eprintln!(
        "Usage:\n  rustmoku-solver solve --record FILE --attacker black|white --nodes N [LIMITS] --checkpoint FILE --output FILE\n  rustmoku-solver resume --checkpoint FILE --nodes N [LIMITS] --output FILE\n  rustmoku-solver verify --book FILE\n  rustmoku-solver inspect --book FILE\n  rustmoku-solver query --book FILE --record FILE\n\nLIMITS: --seconds N --resident-nodes N --vcf-plies N --vcf-nodes N --vct-plies N --vct-nodes N"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stone_parser_is_explicit() {
        assert_eq!(parse_stone("black").unwrap(), Stone::Black);
        assert_eq!(parse_stone("W").unwrap(), Stone::White);
        assert!(parse_stone("first").is_err());
    }

    #[test]
    fn termination_names_remain_distinct() {
        assert_ne!(
            rustmoku_engine::SolverTermination::WorkLimit,
            rustmoku_engine::SolverTermination::TimeLimit
        );
    }
}
