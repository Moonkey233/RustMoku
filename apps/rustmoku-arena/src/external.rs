//! Bounded Gomocup pipe adapter. Only 15x15 Freestyle is supported.

use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rustmoku_core::{Game, Move};

pub struct ExternalPlayer {
    child: Child,
    input: Option<SyncSender<String>>,
    acknowledgements: Receiver<Result<(), String>>,
    writer: Option<JoinHandle<()>>,
    output: Option<Receiver<Result<String, String>>>,
    reader: Option<JoinHandle<()>>,
    started: bool,
}

impl ExternalPlayer {
    pub fn start(
        path: &Path,
        args: &[String],
        clock: Option<Duration>,
        memory: Option<u64>,
    ) -> Result<Self, String> {
        let mut child = Command::new(path)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("spawn: {error}"))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let (input, commands) = mpsc::sync_channel::<String>(1);
        let (acknowledge, acknowledgements) = mpsc::sync_channel(1);
        let writer = thread::spawn(move || {
            while let Ok(command) = commands.recv() {
                let result = write!(stdin, "{command}\r\n")
                    .and_then(|()| stdin.flush())
                    .map_err(|error| format!("protocol write: {error}"));
                let failed = result.is_err();
                if acknowledge.send(result).is_err() || failed {
                    break;
                }
            }
        });
        let stdout = child.stdout.take().expect("piped stdout");
        let (sender, receiver) = mpsc::sync_channel(16);
        let reader = thread::spawn(move || {
            let mut source = BufReader::new(stdout);
            loop {
                let result = read_line(&mut source);
                let done = result.is_err();
                if sender.send(result).is_err() || done {
                    break;
                }
            }
        });
        let mut player = Self {
            child,
            input: Some(input),
            acknowledgements,
            writer: Some(writer),
            output: Some(receiver),
            reader: Some(reader),
            started: false,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        player.send("START 15", deadline)?;
        let reply = player.reply(deadline.saturating_duration_since(Instant::now()))?;
        if reply != "OK" && !reply.starts_with("OK ") {
            return Err(format!("START rejected: {reply}"));
        }
        player.send("INFO rule 0", deadline)?;
        player.send(
            &format!(
                "INFO timeout_match {}",
                clock.map_or(0, |time| time.as_millis())
            ),
            deadline,
        )?;
        if let Some(bytes) = memory {
            player.send(&format!("INFO max_memory {bytes}"), deadline)?;
        }
        Ok(player)
    }

    fn send(&mut self, command: &str, deadline: Instant) -> Result<(), String> {
        self.input
            .as_ref()
            .expect("active writer")
            .send(command.to_owned())
            .map_err(|error| error.to_string())?;
        self.acknowledgements
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| format!("protocol write timeout: {error}"))?
    }

    fn reply(&self, timeout: Duration) -> Result<String, String> {
        let start = Instant::now();
        for _ in 0..128 {
            let remaining = timeout
                .checked_sub(start.elapsed())
                .ok_or("protocol timeout")?;
            let line = self
                .output
                .as_ref()
                .expect("active receiver")
                .recv_timeout(remaining)
                .map_err(|error| format!("protocol timeout/exit: {error}"))??;
            if line.is_empty() || line.starts_with("MESSAGE ") || line.starts_with("DEBUG ") {
                continue;
            }
            return Ok(line);
        }
        Err("protocol output flood".into())
    }

    pub fn choose(
        &mut self,
        game: &Game,
        turn_limit: Duration,
        timeout: Duration,
        clock: Option<Duration>,
    ) -> Result<Move, String> {
        let start = Instant::now();
        let deadline = start
            .checked_add(timeout)
            .ok_or("invalid protocol timeout")?;
        self.send(
            &format!("INFO timeout_turn {}", turn_limit.as_millis()),
            deadline,
        )?;
        // The match clock includes startup and this turn's protocol processing.
        // A manager's soft search allocation is not the remaining match time.
        let time_left = clock.map_or(2_147_483_647, |time| {
            time.saturating_sub(start.elapsed()).as_millis()
        });
        self.send(&format!("INFO time_left {time_left}"), deadline)?;
        if self.started {
            let last = game.history().last().ok_or("missing opponent move")?;
            self.send(&format!("TURN {},{}", last.column(), last.row()), deadline)?;
        } else if game.history().len() == 0 {
            self.send("BEGIN", deadline)?;
        } else {
            self.send("BOARD", deadline)?;
            let own_parity = game.history().len() % 2;
            for (ply, at) in game.history().enumerate() {
                self.send(
                    &format!(
                        "{},{},{}",
                        at.column(),
                        at.row(),
                        if ply % 2 == own_parity { 1 } else { 2 }
                    ),
                    deadline,
                )?;
            }
            self.send("DONE", deadline)?;
        }
        self.started = true;
        let remaining = timeout
            .checked_sub(start.elapsed())
            .ok_or("protocol timeout")?;
        let reply = self.reply(remaining)?;
        if reply == "ERROR" || reply.starts_with("ERROR ") {
            return Err(format!("external {reply}"));
        }
        let at = parse_move(&reply)?;
        if !game.position().is_legal(at) {
            return Err(format!("illegal external move {at}"));
        }
        Ok(at)
    }
}

impl Drop for ExternalPlayer {
    fn drop(&mut self) {
        // Killing closes stdout; dropping the receiver unblocks a full channel.
        // Every reader is joined, including failure and timeout paths.
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.output.take();
        self.input.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn read_line(source: &mut impl BufRead) -> Result<String, String> {
    let mut bytes = Vec::new();
    loop {
        let available = source.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return Err("external stdout closed".into());
        }
        // Do not wait for an optional LF after CR: a CR-only brain is already
        // waiting for the next command. A following LF is an ignored empty line.
        let ending = available
            .iter()
            .position(|&byte| byte == b'\n' || byte == b'\r');
        let count = ending.unwrap_or(available.len());
        if bytes.len() + count > 4096 {
            return Err("external line exceeds 4096 bytes".into());
        }
        bytes.extend_from_slice(&available[..count]);
        source.consume(count + usize::from(ending.is_some()));
        if ending.is_some() {
            return String::from_utf8(bytes)
                .map(|text| text.trim().to_owned())
                .map_err(|error| error.to_string());
        }
    }
}

fn parse_move(text: &str) -> Result<Move, String> {
    let (x, y) = text.split_once(',').ok_or("expected X,Y")?;
    let x: usize = x.trim().parse().map_err(|_| "invalid X")?;
    let y: usize = y.trim().parse().map_err(|_| "invalid Y")?;
    Move::from_row_col(y, x).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_protocol_lines_and_coordinates() {
        assert_eq!(parse_move("7,7").unwrap(), Move::CENTER);
        for text in ["15,0", "0,-1", "7,7,1", "INFO 7,7", "7"] {
            assert!(parse_move(text).is_err());
        }
        assert_eq!(read_line(&mut &b"OK\r\n"[..]).unwrap(), "OK");
        assert!(read_line(&mut vec![b'x'; 4097].as_slice()).is_err());
        assert!(read_line(&mut &b"partial"[..]).is_err());
    }

    #[test]
    fn every_line_ending_and_buffer_split_preserves_complete_messages() {
        for capacity in 1..=9 {
            let mut source = BufReader::with_capacity(capacity, &b"OK\r1,2\r\n3,4\n\r5,6\r"[..]);
            let mut lines = Vec::new();
            while let Ok(line) = read_line(&mut source) {
                if !line.is_empty() {
                    lines.push(line);
                }
            }
            assert_eq!(lines, ["OK", "1,2", "3,4", "5,6"]);
        }
        let mut exact = vec![b'x'; 4096];
        exact.push(b'\r');
        assert_eq!(read_line(&mut exact.as_slice()).unwrap().len(), 4096);
        exact.insert(0, b'x');
        assert!(read_line(&mut exact.as_slice()).is_err());
        assert!(read_line(&mut &b""[..]).is_err());
        assert!(read_line(&mut &[0xff, b'\r'][..]).is_err());
    }
}
