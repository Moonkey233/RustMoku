//! The worker exclusively owns the engine. Only immutable positions, one-way
//! tokens and coarse completed snapshots cross the application boundary.
use std::{
    io,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rustmoku_core::Position;
#[path = "../../time_manager.rs"]
mod time_manager;
use rustmoku_engine::{
    AlphaBetaEngine, CancellationToken, EngineConfig, RuntimeEvaluator, SearchEngine, SearchInfo,
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
    Reconfigure(Box<EngineConfig>),
    ReplaceEvaluator(RuntimeEvaluator),
    OpeningBook(Option<(std::path::PathBuf, rustmoku_engine::OpeningPolicy)>),
    Shutdown,
}

pub(super) enum SearchEvent {
    Info {
        id: u64,
        info: SearchInfo,
    },
    Finished {
        id: u64,
        result: SearchResult,
        elapsed: Duration,
    },
}

pub(super) struct SearchWorker {
    requests: mpsc::Sender<Command>,
    events: mpsc::Receiver<SearchEvent>,
    handle: Option<JoinHandle<()>>,
    request_id: u64,
    cancellation: Option<CancellationToken>,
    book_status: mpsc::Receiver<Result<Option<String>, String>>,
}

impl SearchWorker {
    pub(super) fn new(config: EngineConfig) -> io::Result<Self> {
        let (requests, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        let (book_outgoing, book_status) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("rustmoku-search".into())
            .spawn(move || {
                let mut engine = AlphaBetaEngine::with_config(RuntimeEvaluator::Pattern, config);
                while let Ok(command) = incoming.recv() {
                    match command {
                        Command::Search(request) => {
                            if request.cancellation.is_cancelled() {
                                continue;
                            }
                            let id = request.id;
                            let started = Instant::now();
                            let mut observer = time_manager::ManagedObserver::new(
                                time_manager::TimeManager::new(
                                    None,
                                    Duration::ZERO,
                                    request.limits.move_time,
                                )
                                .with_profile(
                                    engine.effective_search_profile(),
                                    request.position.move_count(),
                                ),
                                |info| {
                                    let _ = outgoing.send(SearchEvent::Info { id, info });
                                },
                            );
                            let result = engine.search_controlled(
                                &request.position,
                                request.limits,
                                request.cancellation,
                                &mut observer,
                            );
                            let elapsed = started.elapsed();
                            if outgoing
                                .send(SearchEvent::Finished {
                                    id,
                                    result,
                                    elapsed,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Command::Reconfigure(config) => {
                            engine.clear_opening_database();
                            engine.reconfigure(*config);
                            let _ = book_outgoing.send(Ok(None));
                        }
                        Command::ReplaceEvaluator(evaluator) => {
                            engine.clear_opening_database();
                            engine.replace_evaluator(evaluator);
                            let _ = book_outgoing.send(Ok(None));
                        }
                        Command::OpeningBook(request) => {
                            // Failed replacement disables the old empirical book.
                            engine.clear_opening_database();
                            let result = request
                                .map(|(path, policy)| {
                                    let database =
                                        rustmoku_engine::OpeningDatabase::read_from_path(&path)
                                            .map_err(|error| error.to_string())?;
                                    let status = format!(
                                        "{} | {:?} | {} entries | {}",
                                        path.display(),
                                        policy,
                                        database.len(),
                                        database.identity().engine_build
                                    );
                                    engine
                                        .set_opening_database(
                                            std::sync::Arc::new(database),
                                            policy,
                                            rustmoku_engine::ENGINE_BUILD_ID,
                                        )
                                        .map_err(str::to_owned)?;
                                    Ok(status)
                                })
                                .transpose();
                            let _ = book_outgoing.send(result);
                        }
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
            book_status,
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
            .send(Command::Reconfigure(Box::new(config)))
            .map_err(|_| "Search worker disconnected.")
    }

    pub(super) fn replace_evaluator(
        &mut self,
        evaluator: RuntimeEvaluator,
    ) -> Result<(), &'static str> {
        self.invalidate();
        self.requests
            .send(Command::ReplaceEvaluator(evaluator))
            .map_err(|_| "Search worker disconnected.")
    }

    pub(super) fn poll(&self) -> Result<SearchEvent, mpsc::TryRecvError> {
        self.events.try_recv()
    }

    pub(super) fn opening_book(
        &mut self,
        request: Option<(std::path::PathBuf, rustmoku_engine::OpeningPolicy)>,
    ) -> Result<(), &'static str> {
        self.invalidate();
        self.requests
            .send(Command::OpeningBook(request))
            .map_err(|_| "Search worker disconnected.")
    }

    pub(super) fn poll_book_status(&self) -> Option<Result<Option<String>, String>> {
        self.book_status.try_iter().last()
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
    #[test]
    fn empirical_book_replacement_failure_and_reconfiguration_disable_it() {
        use rustmoku_engine::{
            Evaluator, OpeningDatabase, OpeningIdentity, OpeningPolicy, ScoreContract,
        };
        let path = std::env::temp_dir().join(format!(
            "rustmoku-native-book-{}.rmopen",
            std::process::id()
        ));
        let config = EngineConfig::new(0);
        let identity = OpeningIdentity {
            engine_build: rustmoku_engine::ENGINE_BUILD_ID.into(),
            model: RuntimeEvaluator::Pattern.model_fingerprint(),
            profile: config.effective_profile(ScoreContract::Pattern),
            generation: "fixture".into(),
        };
        OpeningDatabase::new(identity.clone())
            .unwrap()
            .write_to_path(&path)
            .unwrap();
        let mut worker = SearchWorker::new(config).unwrap();
        worker
            .opening_book(Some((path.clone(), OpeningPolicy::BookMove)))
            .unwrap();
        assert!(
            worker
                .book_status
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .is_some()
        );
        worker.reconfigure(config).unwrap();
        assert!(
            worker
                .book_status
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .is_none()
        );
        let mut wrong = identity;
        wrong.engine_build = "wrong".into();
        OpeningDatabase::new(wrong)
            .unwrap()
            .write_to_path(&path)
            .unwrap();
        worker
            .opening_book(Some((path.clone(), OpeningPolicy::BookMove)))
            .unwrap();
        assert!(
            worker
                .book_status
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }
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

    #[test]
    fn finished_event_carries_worker_measured_elapsed() {
        let mut worker = SearchWorker::new(EngineConfig::new(0)).unwrap();
        worker
            .start(&Position::default(), SearchLimits::new(1))
            .unwrap();
        loop {
            let event = worker.events.recv_timeout(Duration::from_secs(5)).unwrap();
            if let SearchEvent::Finished { elapsed, .. } = event {
                assert!(elapsed <= Duration::from_secs(5));
                break;
            }
        }
    }
}
