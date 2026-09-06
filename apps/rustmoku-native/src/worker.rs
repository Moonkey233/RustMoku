//! The worker exclusively owns the engine. Only immutable positions, one-way
//! tokens and coarse completed snapshots cross the application boundary.
use std::{
    io,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use rustmoku_core::Position;
use rustmoku_engine::{
    AlphaBetaEngine, CancellationToken, EngineConfig, PatternEvaluator, SearchEngine, SearchInfo,
    SearchLimits, SearchResult,
};

struct SearchRequest {
    id: u64,
    position: Position,
    limits: SearchLimits,
    cancellation: CancellationToken,
}

enum Command {
    Search(Box<SearchRequest>),
    Reconfigure(EngineConfig),
    Shutdown,
}

pub(super) enum SearchEvent {
    Info { id: u64, info: SearchInfo },
    Finished { id: u64, result: SearchResult },
}

pub(super) struct SearchWorker {
    requests: mpsc::Sender<Command>,
    events: mpsc::Receiver<SearchEvent>,
    handle: Option<JoinHandle<()>>,
    request_id: u64,
    cancellation: Option<CancellationToken>,
}

impl SearchWorker {
    pub(super) fn new(config: EngineConfig) -> io::Result<Self> {
        let (requests, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("rustmoku-search".into())
            .spawn(move || {
                let mut engine = AlphaBetaEngine::with_config(PatternEvaluator, config);
                while let Ok(command) = incoming.recv() {
                    match command {
                        Command::Search(request) => {
                            if request.cancellation.is_cancelled() {
                                continue;
                            }
                            let id = request.id;
                            let result = engine.search_controlled(
                                &request.position,
                                request.limits,
                                request.cancellation,
                                &mut |info| {
                                    // Unbounded std channels keep cancellation/shutdown from
                                    // waiting on UI consumption. Events occur only at depths.
                                    let _ = outgoing.send(SearchEvent::Info { id, info });
                                },
                            );
                            if outgoing.send(SearchEvent::Finished { id, result }).is_err() {
                                break;
                            }
                        }
                        Command::Reconfigure(config) => engine.reconfigure(config),
                        Command::Shutdown => break,
                    }
                }
            })?;
        Ok(Self {
            requests,
            events,
            handle: Some(handle),
            request_id: 0,
            cancellation: None,
        })
    }

    pub(super) fn invalidate(&mut self) {
        if let Some(token) = self.cancellation.take() {
            token.cancel();
        }
        // Never wrap and accidentally accept an event from an old request.
        self.request_id = self
            .request_id
            .checked_add(1)
            .expect("request id space exhausted");
    }

    pub(super) fn searching(&self) -> bool {
        self.cancellation.is_some()
    }

    pub(super) fn start(
        &mut self,
        position: &Position,
        limits: SearchLimits,
    ) -> Result<(), &'static str> {
        self.invalidate();
        let cancellation = CancellationToken::new();
        self.requests
            .send(Command::Search(Box::new(SearchRequest {
                id: self.request_id,
                // A request owns its snapshot while the UI can start a new game.
                position: position.clone(),
                limits,
                cancellation: cancellation.clone(),
            })))
            .map_err(|_| "Search worker disconnected.")?;
        self.cancellation = Some(cancellation);
        Ok(())
    }

    pub(super) fn reconfigure(&mut self, config: EngineConfig) -> Result<(), &'static str> {
        self.invalidate();
        self.requests
            .send(Command::Reconfigure(config))
            .map_err(|_| "Search worker disconnected.")
    }

    pub(super) fn poll(&self) -> Result<SearchEvent, mpsc::TryRecvError> {
        self.events.try_recv()
    }

    /// Central admission gate for *every* event, including completed results.
    pub(super) fn accept(&mut self, event: &SearchEvent) -> bool {
        let id = match event {
            SearchEvent::Info { id, .. } | SearchEvent::Finished { id, .. } => *id,
        };
        if id != self.request_id || !self.searching() {
            return false;
        }
        if matches!(event, SearchEvent::Finished { .. }) {
            self.cancellation = None;
        }
        true
    }
}

impl Drop for SearchWorker {
    fn drop(&mut self) {
        self.invalidate();
        let _ = self.requests.send(Command::Shutdown);
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            eprintln!("RustMoku search worker panicked.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_worker_cancels_and_joins_cleanly() {
        let mut worker = SearchWorker::new(EngineConfig::new(1)).unwrap();
        worker
            .start(&Position::default(), SearchLimits::new(20))
            .unwrap();
        let event = worker
            .events
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(worker.accept(&event));
        assert!(worker.searching());
        let token = worker.cancellation.as_ref().unwrap().clone();
        let before = std::time::Instant::now();
        let (_, replacement) = mpsc::channel();
        let events = std::mem::replace(&mut worker.events, replacement);
        drop(worker);
        // The real Drop path has joined: after queued snapshots are drained,
        // the producer must already be disconnected, not merely idle.
        loop {
            match events.try_recv() {
                Ok(_) => {}
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => panic!("worker still alive after drop"),
            }
        }
        assert!(token.is_cancelled());
        assert!(before.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn reconfigure_invalidates_old_events_and_applies_on_the_owner_thread() {
        let mut worker = SearchWorker::new(EngineConfig::new(1)).unwrap();
        worker
            .start(&Position::default(), SearchLimits::new(20))
            .unwrap();
        let old = worker
            .events
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(worker.accept(&old));
        worker
            .reconfigure(EngineConfig::new(0).with_threads(2))
            .unwrap();
        assert!(!worker.searching());
        worker
            .start(&Position::default(), SearchLimits::new(1))
            .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let event = worker
                .events
                .recv_timeout(std::time::Duration::from_millis(100))
                .unwrap();
            if !worker.accept(&event) {
                assert!(std::time::Instant::now() < until);
                continue;
            }
            if let SearchEvent::Finished { result, .. } = event {
                assert_eq!(result.statistics.worker_count, 2);
                break;
            }
            assert!(std::time::Instant::now() < until);
        }
    }
}
