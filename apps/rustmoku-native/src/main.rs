#![forbid(unsafe_code)]

use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use rustmoku_core::{BOARD_SIZE, Game, GameStatus, Move, MoveError, OPENINGS, RecordError, Stone};
use rustmoku_engine::{
    EngineConfig, Evaluator, RuntimeEvaluator, ScoreContract, SearchInfo, SearchLimits,
    SearchProfile, SearchTermination,
};
use std::{
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};
mod localization;
mod worker;
use localization::{LanguagePreference, TextKey, UiLanguage, UiText, install_windows_cjk_font};
use worker::{SearchEvent, SearchWorker};

const BOARD_MARGIN: f32 = 28.0;
const BOARD_COLOR: Color32 = Color32::from_rgb(216, 171, 103);
const GRID_COLOR: Color32 = Color32::from_rgb(63, 45, 28);
const LAST_MOVE_COLOR: Color32 = Color32::from_rgb(210, 48, 42);
const HISTORY_PANEL_WIDTH: f32 = 300.0;
const NATIVE_DEPTH: u8 = 8;
const NATIVE_MAX_AUTO_THREADS: usize = 8;
const NATIVE_TT_MEMORY_MIB: usize = 128;
const NATIVE_MOVE_TIME_MS: u64 = 15_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeDefaults {
    depth: u8,
    threads: usize,
    tt_memory_mib: usize,
    move_time_ms: u64,
    show_move_numbers: bool,
    language: LanguagePreference,
}

fn evaluator_identity(evaluator: &RuntimeEvaluator) -> String {
    let fingerprint = evaluator.model_fingerprint().map_or_else(
        || "unavailable".to_owned(),
        |bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
    );
    format!(
        "{} | format {:?} | {:?}\n{}",
        evaluator.architecture_name(),
        evaluator.model_format_version(),
        evaluator.score_contract(),
        fingerprint
    )
}

fn auto_thread_count(available: usize) -> usize {
    available.clamp(1, NATIVE_MAX_AUTO_THREADS)
}

fn host_thread_count() -> usize {
    std::thread::available_parallelism().map_or(1, |count| count.get())
}

fn native_defaults(available_threads: usize) -> NativeDefaults {
    NativeDefaults {
        depth: NATIVE_DEPTH,
        threads: auto_thread_count(available_threads),
        tt_memory_mib: NATIVE_TT_MEMORY_MIB,
        move_time_ms: NATIVE_MOVE_TIME_MS,
        show_move_numbers: true,
        language: LanguagePreference::Auto,
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 880.0])
            .with_min_inner_size([720.0, 650.0]),
        ..Default::default()
    };

    let title = format!("RustMoku V{}", env!("CARGO_PKG_VERSION"));
    eframe::run_native(
        &title,
        options,
        Box::new(|creation_context| Ok(Box::new(RustMokuApp::new(&creation_context.egui_ctx)?))),
    )
}

#[derive(Debug, Default)]
struct MoveTimings {
    history: Vec<Option<Duration>>,
    future: Vec<Option<Duration>>,
}

impl MoveTimings {
    fn reset(&mut self, plies: usize) {
        self.history = vec![None; plies];
        self.future.clear();
    }

    fn align_unknown_history(&mut self, plies: usize) {
        if self.history.len() < plies {
            self.history.resize(plies, None);
        }
    }

    fn record_new(&mut self, elapsed: Option<Duration>) {
        self.history.push(elapsed);
        self.future.clear();
    }

    fn undo_plies(&mut self, plies: usize) {
        for _ in 0..plies.min(self.history.len()) {
            self.future.push(self.history.pop().unwrap());
        }
    }

    fn redo(&mut self) {
        if let Some(elapsed) = self.future.pop() {
            self.history.push(elapsed);
        }
    }
}

struct RustMokuApp {
    game: Game,
    human_stone: Stone,
    worker: SearchWorker,
    engine_config: EngineConfig,
    threads_auto: bool,
    manual_threads: usize,
    search_limits: SearchLimits,
    last_search: Option<SearchInfo>,
    move_time_ms: u64,
    message: Option<String>,
    selected_opening: Option<usize>,
    undo_floor: usize,
    show_move_numbers: bool,
    record_open: bool,
    record_text: String,
    record_path: String,
    language_preference: LanguagePreference,
    text: UiText,
    cjk_font_installed: bool,
    timings: MoveTimings,
    turn_started: Option<Instant>,
    model_path: String,
    profile_path: String,
    loaded_contract: ScoreContract,
    loaded_model: Option<String>,
    loaded_identity: String,
    book_path: String,
    book_policy: rustmoku_engine::OpeningPolicy,
    book_status: Option<String>,
}

impl RustMokuApp {
    fn new(ctx: &egui::Context) -> std::io::Result<Self> {
        let defaults = native_defaults(host_thread_count());
        let config = EngineConfig::default()
            .with_threads(defaults.threads)
            .with_tt_memory_mib(defaults.tt_memory_mib);
        let mut app = Self::with_worker_config(SearchWorker::new(config)?, config, true);
        app.apply_language(ctx, defaults.language);
        Ok(app)
    }

    #[cfg(test)]
    fn with_worker(worker: SearchWorker) -> Self {
        Self::with_worker_config(worker, EngineConfig::default(), false)
    }

    fn with_worker_config(
        worker: SearchWorker,
        engine_config: EngineConfig,
        threads_auto: bool,
    ) -> Self {
        let defaults = native_defaults(engine_config.threads());
        Self {
            game: Game::default(),
            human_stone: Stone::Black,
            worker,
            engine_config,
            threads_auto,
            manual_threads: engine_config.threads(),
            move_time_ms: defaults.move_time_ms,
            search_limits: SearchLimits::new(defaults.depth),
            last_search: None,
            message: None,
            selected_opening: None,
            undo_floor: 0,
            show_move_numbers: defaults.show_move_numbers,
            record_open: false,
            record_text: String::new(),
            record_path: String::from("rustmoku-game.rmk"),
            language_preference: LanguagePreference::Auto,
            text: UiText::new(UiLanguage::English),
            cjk_font_installed: false,
            timings: MoveTimings::default(),
            turn_started: Some(Instant::now()),
            model_path: String::from("rustmoku-model.rmlp"),
            profile_path: String::from("rustmoku.profile"),
            loaded_contract: ScoreContract::Pattern,
            loaded_model: None,
            loaded_identity: evaluator_identity(&RuntimeEvaluator::Pattern),
            book_path: String::from("openings.rmob"),
            book_policy: rustmoku_engine::OpeningPolicy::OrderOnly,
            book_status: None,
        }
    }

    fn apply_language(&mut self, ctx: &egui::Context, preference: LanguagePreference) {
        let language = preference.resolve(sys_locale::get_locale().as_deref());
        self.language_preference = preference;
        if language == UiLanguage::SimplifiedChinese
            && !self.cjk_font_installed
            && !install_windows_cjk_font(ctx)
        {
            self.text = UiText::new(UiLanguage::English);
            self.message = Some(self.text.get(TextKey::CjkFontMissing).into());
            return;
        }
        self.cjk_font_installed |= language == UiLanguage::SimplifiedChinese;
        self.text = UiText::new(language);
    }

    fn replace_game(&mut self, game: Game, undo_floor: usize) {
        self.worker.invalidate();
        self.game = game;
        self.timings.reset(self.game.history().len());
        self.undo_floor = undo_floor;
        self.last_search = None;
        self.message = None;
        self.turn_started = None;
        self.play_ai_if_needed();
        self.start_human_timer_if_needed();
    }

    fn new_game(&mut self) {
        let game = self
            .selected_opening
            .map_or_else(|| Ok(Game::default()), |index| OPENINGS[index].game());
        match game {
            Ok(game) => {
                let floor = game.history().len();
                self.replace_game(game, floor);
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn import_record(&mut self, text: &str) -> Result<(), RecordError> {
        let game = Game::from_record(text)?;
        // Imported records are editable from the start; only an explicitly
        // selected built-in opening sets a session undo floor.
        self.replace_game(game, 0);
        Ok(())
    }

    fn human_undo_plies(&self) -> usize {
        self.game
            .history()
            .enumerate()
            .rev()
            .find(|&(index, at)| {
                index >= self.undo_floor && self.game.position().cell(at) == Some(self.human_stone)
            })
            .map_or(0, |(index, _)| self.game.history().len() - index)
    }

    fn undo_to_human(&mut self) {
        let plies = self.human_undo_plies();
        if plies == 0 {
            return;
        }
        self.worker.invalidate();
        self.timings
            .align_unknown_history(self.game.history().len());
        let undone = self.game.undo_plies(plies);
        self.timings.undo_plies(undone);
        self.last_search = None;
        self.message = None;
        self.turn_started = None;
        self.play_ai_if_needed();
        self.start_human_timer_if_needed();
    }

    fn redo_to_human(&mut self) {
        if !self.game.can_redo() {
            return;
        }
        self.worker.invalidate();
        self.last_search = None;
        self.message = None;
        self.turn_started = None;
        while self.game.can_redo() {
            self.game.redo();
            self.timings.redo();
            if self.game.status() != GameStatus::Ongoing
                || self.game.position().side_to_move() == self.human_stone
                || !self.game.can_redo()
            {
                break;
            }
        }
        self.play_ai_if_needed();
        self.start_human_timer_if_needed();
    }

    fn play_human_move(&mut self, at: Move) {
        if self.game.status() != GameStatus::Ongoing
            || self.game.position().side_to_move() != self.human_stone
        {
            return;
        }

        match self.game.play_move(at) {
            Ok(()) => {
                let elapsed = self.turn_started.map(|started| started.elapsed());
                self.timings.record_new(elapsed);
                self.turn_started = None;
                self.worker.invalidate();
                self.message = None;
                self.play_ai_if_needed();
            }
            Err(MoveError::Occupied { .. }) => {
                self.message = Some(self.text.get(TextKey::Occupied).into());
            }
            Err(error) => {
                self.message = Some(self.text.detail(TextKey::MoveFailed, &error.to_string()));
            }
        }
    }

    fn play_ai_if_needed(&mut self) {
        if self.worker.searching()
            || self.game.status() != GameStatus::Ongoing
            || self.game.position().side_to_move() == self.human_stone
        {
            return;
        }

        self.last_search = None;
        let limits = if self.move_time_ms == 0 {
            self.search_limits
        } else {
            self.search_limits
                .with_move_time(Duration::from_millis(self.move_time_ms))
        };
        if let Err(error) = self.worker.start(self.game.position(), limits) {
            self.message = Some(self.text.detail(TextKey::SearchFailed, error));
            self.turn_started = None;
        } else {
            self.turn_started = Some(Instant::now());
        }
    }

    fn start_human_timer_if_needed(&mut self) {
        if self.turn_started.is_none()
            && self.game.status() == GameStatus::Ongoing
            && self.game.position().side_to_move() == self.human_stone
        {
            self.turn_started = Some(Instant::now());
        }
    }

    fn use_pattern_evaluator(&mut self) {
        if let Err(error) = self.worker.replace_evaluator(RuntimeEvaluator::Pattern) {
            self.message = Some(self.text.detail(TextKey::ReconfigureFailed, error));
        } else {
            self.loaded_contract = ScoreContract::Pattern;
            self.loaded_model = None;
            self.loaded_identity = evaluator_identity(&RuntimeEvaluator::Pattern);
            self.last_search = None;
            self.message = None;
            self.turn_started = None;
            self.play_ai_if_needed();
            self.start_human_timer_if_needed();
        }
    }

    fn load_search_profile(&mut self) {
        let result = (|| -> Result<SearchProfile, String> {
            let metadata =
                std::fs::metadata(&self.profile_path).map_err(|error| error.to_string())?;
            if metadata.len() > 1024 {
                return Err("search profile exceeds 1024 bytes".into());
            }
            let text =
                std::fs::read_to_string(&self.profile_path).map_err(|error| error.to_string())?;
            let profile: SearchProfile = text.parse().map_err(str::to_string)?;
            if profile.contract() != self.loaded_contract {
                return Err("search profile does not match loaded evaluator".into());
            }
            Ok(profile)
        })();
        match result {
            Ok(profile) => {
                let config = self.engine_config.with_search_profile(profile);
                match self.worker.reconfigure(config) {
                    Ok(()) => {
                        self.engine_config = config;
                        self.last_search = None;
                        self.message = None;
                        self.play_ai_if_needed();
                    }
                    Err(error) => {
                        self.message = Some(self.text.detail(TextKey::ReconfigureFailed, error))
                    }
                }
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn load_learned_model(&mut self) {
        match RuntimeEvaluator::read_from_path(&self.model_path) {
            Ok(evaluator) => {
                let loaded_contract = evaluator.score_contract();
                let identity = evaluator_identity(&evaluator);
                self.last_search = None;
                if let Err(error) = self.worker.replace_evaluator(evaluator) {
                    self.message = Some(self.text.detail(TextKey::ReconfigureFailed, error));
                    return;
                }
                self.loaded_contract = loaded_contract;
                self.loaded_identity = identity;
                self.loaded_model = Some(self.model_path.clone());
                self.message = None;
                self.turn_started = None;
                self.play_ai_if_needed();
                self.start_human_timer_if_needed();
            }
            Err(error) => {
                // A rejected file must not silently replace the active model.
                self.message = Some(self.text.detail(TextKey::LoadModel, &error.to_string()));
            }
        }
    }

    fn handle_event(&mut self, event: SearchEvent) {
        if !self.worker.accept(&event) {
            return;
        }
        match event {
            SearchEvent::Info { info, .. } => self.last_search = Some(info),
            SearchEvent::Finished {
                result, elapsed, ..
            } => {
                self.last_search = Some(SearchInfo::from(&result));
                if result.termination == SearchTermination::Cancelled {
                    return;
                }
                if self.game.status() != GameStatus::Ongoing
                    || self.game.position().side_to_move() == self.human_stone
                {
                    return;
                }
                let Some(at) = result.best_move else {
                    self.message = Some(self.text.get(TextKey::NoLegalMove).into());
                    return;
                };
                if let Err(error) = self.game.play_move(at) {
                    self.message = Some(
                        self.text
                            .detail(TextKey::EngineMoveFailed, &error.to_string()),
                    );
                } else {
                    self.timings.record_new(Some(elapsed));
                    self.turn_started = None;
                    self.start_human_timer_if_needed();
                }
            }
        }
    }

    fn poll_search(&mut self) {
        loop {
            match self.worker.poll() {
                Ok(event) => self.handle_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.worker.searching() {
                        self.worker.invalidate();
                        self.message = Some(self.text.get(TextKey::WorkerDisconnected).into());
                    }
                    break;
                }
            }
        }
    }

    fn status_text(&self) -> String {
        let (won, draw) = match self.game.status() {
            GameStatus::Won(stone) => (Some(stone), false),
            GameStatus::Draw => (None, true),
            GameStatus::Ongoing => (None, false),
        };
        self.text.status(
            won,
            draw,
            self.human_stone,
            self.game.position().side_to_move(),
        )
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let text = self.text;
        ui.heading(format!("RustMoku V{}", env!("CARGO_PKG_VERSION")));
        ui.horizontal(|ui| {
            ui.label(text.get(TextKey::Language));
            let previous = self.language_preference;
            egui::ComboBox::from_id_salt("language")
                .selected_text(match self.language_preference {
                    LanguagePreference::Auto => text.get(TextKey::Auto),
                    LanguagePreference::English => text.get(TextKey::English),
                    LanguagePreference::SimplifiedChinese => text.get(TextKey::SimplifiedChinese),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.language_preference,
                        LanguagePreference::Auto,
                        text.get(TextKey::Auto),
                    );
                    ui.selectable_value(
                        &mut self.language_preference,
                        LanguagePreference::English,
                        text.get(TextKey::English),
                    );
                    ui.selectable_value(
                        &mut self.language_preference,
                        LanguagePreference::SimplifiedChinese,
                        text.get(TextKey::SimplifiedChinese),
                    );
                });
            if self.language_preference != previous {
                self.apply_language(ui.ctx(), self.language_preference);
            }
        });
        let text = self.text;
        ui.horizontal(|ui| {
            ui.label(text.get(TextKey::PlayAs));
            let previous_stone = self.human_stone;
            ui.radio_value(
                &mut self.human_stone,
                Stone::Black,
                text.get(TextKey::Black),
            );
            ui.radio_value(
                &mut self.human_stone,
                Stone::White,
                text.get(TextKey::White),
            );
            if self.human_stone != previous_stone {
                self.new_game();
            }
            if ui.button(text.get(TextKey::NewGame)).clicked() {
                self.new_game();
            }
            if ui
                .add_enabled(
                    self.human_undo_plies() > 0,
                    egui::Button::new(text.get(TextKey::UndoTurn)),
                )
                .clicked()
            {
                self.undo_to_human();
            }
            if ui
                .add_enabled(
                    self.game.can_redo(),
                    egui::Button::new(text.get(TextKey::RedoTurn)),
                )
                .clicked()
            {
                self.redo_to_human();
            }
            if ui.button(text.get(TextKey::GameRecord)).clicked() {
                self.record_text = self.game.to_record();
                self.record_open = true;
            }
            ui.checkbox(&mut self.show_move_numbers, text.get(TextKey::MoveNumbers));
        });
        ui.horizontal(|ui| {
            ui.label(text.get(TextKey::Opening));
            let previous = self.selected_opening;
            egui::ComboBox::from_id_salt("opening")
                .selected_text(
                    self.selected_opening
                        .map_or(text.get(TextKey::EmptyBoard), |i| OPENINGS[i].name),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.selected_opening,
                        None,
                        text.get(TextKey::EmptyBoard),
                    );
                    for (index, opening) in OPENINGS.iter().enumerate() {
                        ui.selectable_value(&mut self.selected_opening, Some(index), opening.name);
                    }
                });
            if ui.button(text.get(TextKey::NextInSuite)).clicked() {
                self.selected_opening = Some(
                    self.selected_opening
                        .map_or(0, |i| (i + 1) % OPENINGS.len()),
                );
            }
            if self.selected_opening != previous {
                self.new_game();
            }
            ui.small(text.get(TextKey::OpeningNote));
        });
        ui.horizontal(|ui| {
            ui.label(text.get(TextKey::NextDepth));
            ui.add(egui::DragValue::new(&mut self.search_limits.max_depth).range(1..=12));
            ui.label(text.get(TextKey::MoveTime));
            ui.add(egui::DragValue::new(&mut self.move_time_ms).range(0..=60_000));
            ui.small(text.get(TextKey::UnlimitedHint));
            if self.worker.searching() {
                ui.spinner();
            }
            if let Some(started) = self.turn_started {
                ui.label(text.current_move_time(started.elapsed()));
            }
            if let Some(at) = self.game.position().last_move() {
                ui.label(text.last_move(at, self.timings.history.last().copied().flatten()));
            }
        });
        ui.horizontal(|ui| {
            ui.label(text.get(TextKey::Evaluator));
            ui.label(
                self.loaded_model
                    .as_deref()
                    .unwrap_or_else(|| text.get(TextKey::PatternEval)),
            );
            ui.label(text.get(TextKey::ModelPath));
            ui.text_edit_singleline(&mut self.model_path);
            if ui.button(text.get(TextKey::LoadModel)).clicked() {
                self.load_learned_model();
            }
            if ui.button(text.get(TextKey::UsePattern)).clicked() {
                self.use_pattern_evaluator();
            }
        });
        ui.horizontal(|ui| {
            ui.label(text.get(TextKey::SearchProfile));
            ui.text_edit_singleline(&mut self.profile_path);
            if ui.button(text.get(TextKey::LoadProfile)).clicked() {
                self.load_search_profile();
            }
            let effective = self.engine_config.effective_profile(self.loaded_contract);
            ui.label(if effective.policy_lmr() || effective.singular() {
                text.get(TextKey::ResearchProfile)
            } else {
                text.get(TextKey::BaselineProfile)
            });
        });
        let previous_threads = self.engine_config.threads();
        if let Some(status) = self.worker.poll_book_status() {
            match status {
                Ok(status) => self.book_status = status,
                Err(error) => {
                    self.book_status = None;
                    self.message = Some(error);
                }
            }
        }
        ui.collapsing(text.get(TextKey::EmpiricalBook), |ui| {
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.book_path);
                ui.selectable_value(
                    &mut self.book_policy,
                    rustmoku_engine::OpeningPolicy::OrderOnly,
                    "OrderOnly",
                );
                ui.selectable_value(
                    &mut self.book_policy,
                    rustmoku_engine::OpeningPolicy::BookMove,
                    "BookMove",
                );
                if ui.button(text.get(TextKey::LoadBook)).clicked() {
                    self.book_status = None;
                    if let Err(error) = self
                        .worker
                        .opening_book(Some((self.book_path.clone().into(), self.book_policy)))
                    {
                        self.message = Some(error.into());
                    }
                    self.last_search = None;
                    self.play_ai_if_needed();
                }
                if ui.button(text.get(TextKey::ClearBook)).clicked() {
                    self.book_status = None;
                    if let Err(error) = self.worker.opening_book(None) {
                        self.message = Some(error.into());
                    }
                    self.last_search = None;
                    self.play_ai_if_needed();
                }
            });
            ui.label(
                self.book_status
                    .as_deref()
                    .unwrap_or(text.get(TextKey::BookDisabled)),
            );
        });
        let previous_auto = self.threads_auto;
        let previous_tt_memory = self.engine_config.tt_memory_mib();
        let mut tt_memory_mib = previous_tt_memory;
        ui.horizontal(|ui| {
            ui.label(text.get(TextKey::Threads));
            ui.radio_value(
                &mut self.threads_auto,
                true,
                format!(
                    "{} ({})",
                    text.get(TextKey::Auto),
                    auto_thread_count(host_thread_count())
                ),
            );
            ui.radio_value(&mut self.threads_auto, false, text.get(TextKey::Manual));
            ui.add_enabled(
                !self.threads_auto,
                egui::DragValue::new(&mut self.manual_threads).range(1..=host_thread_count()),
            );
            ui.label(text.get(TextKey::TtPrimary));
            ui.add(egui::DragValue::new(&mut tt_memory_mib).range(1..=4096));
        });
        let threads = if self.threads_auto {
            auto_thread_count(host_thread_count())
        } else {
            self.manual_threads.clamp(1, host_thread_count())
        };
        if self.threads_auto != previous_auto
            || threads != previous_threads
            || tt_memory_mib != previous_tt_memory
        {
            let config = self
                .engine_config
                .with_threads(threads)
                .with_tt_memory_mib(tt_memory_mib);
            self.engine_config = config;
            self.last_search = None;
            if let Err(error) = self.worker.reconfigure(config) {
                self.message = Some(text.detail(TextKey::ReconfigureFailed, error));
            } else {
                // Reconfiguration invalidates the old request. If the restored
                // game still needs an AI move, start it after the command so the
                // worker processes FIFO: reconfigure, then replacement search.
                self.play_ai_if_needed();
            }
        }
        ui.horizontal(|ui| {
            ui.strong(self.status_text());
            if let Some(message) = &self.message {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(message).color(Color32::from_rgb(190, 45, 40)),
                    )
                    .truncate(),
                )
                .on_hover_text(message);
            }
        });
        if let Some(search) = &self.last_search {
            ui.add(
                egui::Label::new(text.search_summary(
                    search.completed_depth,
                    search.seldepth,
                    search.statistics.work_nodes,
                    search.statistics.qnodes,
                    search.statistics.worker_count,
                    search.score,
                ))
                .truncate(),
            );
            ui.add(
                egui::Label::new(text.tt_summary(
                    search.statistics.tt_hits,
                    search.statistics.tt_cutoffs,
                    search.statistics.tt_probes,
                    search.statistics.tt_stores,
                ))
                .truncate(),
            );
            ui.label(text.result_summary(search));
        } else {
            ui.label(text.get(TextKey::Waiting));
            ui.label(text.get(TextKey::TtEmpty));
            ui.label(if self.worker.searching() {
                text.get(TextKey::Searching)
            } else {
                text.get(TextKey::Waiting)
            });
        }
        ui.collapsing(text.get(TextKey::Diagnostics), |ui| {
            ui.monospace(&self.loaded_identity);
            ui.monospace(
                self.engine_config
                    .effective_profile(self.loaded_contract)
                    .to_string(),
            );
            if let Some(search) = &self.last_search {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (name, count) in search
                            .statistics
                            .counters()
                            .filter(|(_, count)| *count != 0)
                        {
                            ui.monospace(format!("{name}: {count}"));
                        }
                    });
            }
        });
        egui::ScrollArea::horizontal()
            .id_salt("pv")
            .max_height(28.0)
            .min_scrolled_height(28.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.monospace(text.get(TextKey::Pv));
                    if let Some(search) = &self.last_search {
                        for at in &search.principal_variation {
                            ui.monospace(at.to_string());
                        }
                    }
                });
            });
    }

    fn move_history(&self, ui: &mut egui::Ui) {
        let text = self.text;
        ui.heading(text.get(TextKey::Moves));
        ui.small(text.undo_floor(self.undo_floor));
        egui::ScrollArea::both()
            .id_salt("history")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("move_list").striped(true).show(ui, |ui| {
                    ui.label("#");
                    ui.label(text.get(TextKey::Black));
                    ui.label(text.get(TextKey::Time));
                    ui.label(text.get(TextKey::White));
                    ui.label(text.get(TextKey::Time));
                    ui.end_row();
                    let moves: Vec<_> = self.game.history().collect();
                    for (turn_index, pair) in moves.chunks(2).enumerate() {
                        let turn = turn_index + 1;
                        ui.label(turn.to_string());
                        let black = pair[0];
                        ui.monospace(black.to_string());
                        ui.monospace(text.history_time(
                            self.timings.history.get((turn - 1) * 2).copied().flatten(),
                        ));
                        ui.monospace(
                            pair.get(1)
                                .map_or_else(String::new, |white| white.to_string()),
                        );
                        ui.monospace(
                            text.history_time(
                                self.timings
                                    .history
                                    .get((turn - 1) * 2 + 1)
                                    .copied()
                                    .flatten(),
                            ),
                        );
                        ui.end_row();
                    }
                });
            });
    }

    fn record_window(&mut self, ctx: &egui::Context) {
        if !self.record_open {
            return;
        }
        let mut open = self.record_open;
        let text = self.text;
        egui::Window::new(text.get(TextKey::GameRecord))
            .open(&mut open)
            .default_width(590.0)
            .show(ctx, |ui| {
                ui.label(text.get(TextKey::RecordHelp));
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.record_text)
                                .font(egui::TextStyle::Monospace)
                                .desired_rows(7)
                                .desired_width(f32::INFINITY),
                        );
                    });
                ui.horizontal(|ui| {
                    if ui.button(text.get(TextKey::ExportGame)).clicked() {
                        self.record_text = self.game.to_record();
                    }
                    if ui.button(text.get(TextKey::CopyText)).clicked() {
                        ctx.copy_text(self.record_text.clone());
                    }
                    if ui.button(text.get(TextKey::ImportText)).clicked() {
                        let text = self.record_text.clone();
                        if let Err(error) = self.import_record(&text) {
                            self.message =
                                Some(self.text.detail(TextKey::ImportFailed, &error.to_string()));
                        } else {
                            self.message = Some(self.text.get(TextKey::RecordImported).into());
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(text.get(TextKey::File));
                    ui.text_edit_singleline(&mut self.record_path);
                });
                ui.horizontal(|ui| {
                    if ui.button(text.get(TextKey::LoadFile)).clicked() {
                        match std::fs::read_to_string(&self.record_path) {
                            Ok(text) => {
                                self.record_text = text;
                                self.message = None;
                            }
                            Err(error) => {
                                self.message =
                                    Some(self.text.detail(TextKey::LoadFailed, &error.to_string()));
                            }
                        }
                    }
                    if ui.button(text.get(TextKey::SaveFile)).clicked() {
                        // Validate before writing; output is canonical record syntax.
                        match Game::from_record(&self.record_text) {
                            Ok(game) => match std::fs::write(&self.record_path, game.to_record()) {
                                Ok(()) => {
                                    self.message = Some(self.text.get(TextKey::RecordSaved).into());
                                }
                                Err(error) => {
                                    self.message = Some(
                                        self.text.detail(TextKey::SaveFailed, &error.to_string()),
                                    );
                                }
                            },
                            Err(error) => {
                                self.message = Some(
                                    self.text.detail(TextKey::InvalidRecord, &error.to_string()),
                                );
                            }
                        }
                    }
                });
                if let Some(message) = &self.message {
                    ui.label(message);
                }
            });
        self.record_open = open;
    }

    fn board(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let side = available.x.min(available.y).max(2.0 * BOARD_MARGIN + 14.0);
        let (response, painter) = ui.allocate_painter(Vec2::splat(side), Sense::click());
        let rect = response.rect;
        painter.rect_filled(rect, 6.0, BOARD_COLOR);

        let origin = rect.min + Vec2::splat(BOARD_MARGIN);
        let grid_span = side - 2.0 * BOARD_MARGIN;
        let spacing = grid_span / (BOARD_SIZE - 1) as f32;
        let grid_end = origin + Vec2::splat(grid_span);

        for index in 0..BOARD_SIZE {
            let offset = index as f32 * spacing;
            painter.line_segment(
                [
                    Pos2::new(origin.x, origin.y + offset),
                    Pos2::new(grid_end.x, origin.y + offset),
                ],
                Stroke::new(1.2, GRID_COLOR),
            );
            painter.line_segment(
                [
                    Pos2::new(origin.x + offset, origin.y),
                    Pos2::new(origin.x + offset, grid_end.y),
                ],
                Stroke::new(1.2, GRID_COLOR),
            );
        }

        // Derive axis labels from the shared Move formatter as well. Labels
        // occupy the existing margin; origin/spacing and click mapping agree.
        for index in 0..BOARD_SIZE {
            let column = Move::from_row_col(14, index)
                .expect("board column")
                .to_string();
            let row = Move::from_row_col(index, 0).expect("board row").to_string();
            painter.text(
                Pos2::new(origin.x + index as f32 * spacing, grid_end.y + 17.0),
                egui::Align2::CENTER_CENTER,
                &column[..1],
                egui::FontId::proportional(12.0),
                GRID_COLOR,
            );
            painter.text(
                Pos2::new(origin.x - 17.0, origin.y + index as f32 * spacing),
                egui::Align2::CENTER_CENTER,
                &row[1..],
                egui::FontId::proportional(12.0),
                GRID_COLOR,
            );
        }

        for row in [3, 7, 11] {
            for column in [3, 7, 11] {
                painter.circle_filled(board_point(origin, spacing, row, column), 3.5, GRID_COLOR);
            }
        }

        let stone_radius = spacing * 0.42;
        for at in Move::all() {
            let Some(stone) = self.game.position().cell(at) else {
                continue;
            };
            let center = board_point(origin, spacing, at.row(), at.column());
            match stone {
                Stone::Black => {
                    painter.circle_filled(center, stone_radius, Color32::from_rgb(24, 24, 24));
                }
                Stone::White => {
                    painter.circle_filled(center, stone_radius, Color32::from_rgb(245, 243, 235));
                    painter.circle_stroke(center, stone_radius, Stroke::new(1.2, GRID_COLOR));
                }
            }
            if self.game.position().last_move() == Some(at) {
                painter.circle_stroke(
                    center,
                    stone_radius * 0.36,
                    Stroke::new(2.2, LAST_MOVE_COLOR),
                );
            }
        }

        if self.show_move_numbers {
            for (index, at) in self.game.history().enumerate() {
                let color = if self.game.position().cell(at) == Some(Stone::Black) {
                    Color32::WHITE
                } else {
                    Color32::BLACK
                };
                painter.text(
                    board_point(origin, spacing, at.row(), at.column()),
                    egui::Align2::CENTER_CENTER,
                    (index + 1).to_string(),
                    egui::FontId::proportional((spacing * 0.35).min(16.0)),
                    color,
                );
            }
        }

        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(at) = pointer_to_move(pointer, origin, spacing)
        {
            self.play_human_move(at);
        }
    }
}

impl eframe::App for RustMokuApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_search();
        if self.turn_started.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        // Stable panel sizes isolate board geometry from PV/proof/history text.
        egui::Panel::top("controls")
            .exact_size(300.0)
            .resizable(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("controls_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.controls(ui));
            });
        egui::Panel::right("history_panel")
            // Five columns need room for both localized color names and
            // fixed-precision timings. Keep a horizontal scrollbar as a
            // fallback for unusually long session durations.
            .exact_size(HISTORY_PANEL_WIDTH)
            .resizable(false)
            .show(ui, |ui| self.move_history(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| self.board(ui));
        });
        self.record_window(ui.ctx());
    }
}

fn pointer_to_move(pointer: Pos2, origin: Pos2, spacing: f32) -> Option<Move> {
    let row = nearest_coordinate(pointer.y, origin.y, spacing)?;
    let column = nearest_coordinate(pointer.x, origin.x, spacing)?;
    Move::from_row_col(row, column).ok()
}

fn nearest_coordinate(value: f32, origin: f32, spacing: f32) -> Option<usize> {
    let coordinate = ((value - origin) / spacing).round();
    let maximum = (BOARD_SIZE - 1) as f32;
    if coordinate < 0.0 || coordinate > maximum {
        return None;
    }
    let snapped = origin + coordinate * spacing;
    ((value - snapped).abs() <= spacing * 0.46).then_some(coordinate as usize)
}

fn board_point(origin: Pos2, spacing: f32, row: usize, column: usize) -> Pos2 {
    Pos2::new(
        origin.x + column as f32 * spacing,
        origin.y + row as f32 * spacing,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustmoku_engine::{AlphaBetaEngine, SearchEngine};

    fn test_app() -> RustMokuApp {
        RustMokuApp::with_worker(
            SearchWorker::new(EngineConfig::new(1).with_vct_table_memory(1)).unwrap(),
        )
    }

    fn play_first_legal(game: &mut Game) {
        let at = Move::all()
            .find(|&at| game.position().is_legal(at))
            .unwrap();
        game.play_move(at).unwrap();
    }

    #[test]
    fn native_auto_threads_and_playing_defaults_are_bounded_and_separate() {
        assert_eq!(auto_thread_count(1), 1);
        assert_eq!(auto_thread_count(4), 4);
        assert_eq!(auto_thread_count(8), 8);
        assert_eq!(auto_thread_count(16), 8);

        let defaults = native_defaults(16);
        assert_eq!(defaults.depth, 8);
        assert_eq!(defaults.threads, 8);
        assert_eq!(defaults.tt_memory_mib, 128);
        assert_eq!(defaults.move_time_ms, 15_000);
        assert!(defaults.show_move_numbers);
        assert_eq!(defaults.language, LanguagePreference::Auto);

        assert_eq!(SearchLimits::default().max_depth, 4);
        assert_eq!(EngineConfig::default().threads(), 1);
        assert_eq!(EngineConfig::default().tt_memory_mib(), 64);
    }

    #[test]
    fn undo_returns_to_human_decisions_without_crossing_opening_floor() {
        for opening in [&OPENINGS[0], &OPENINGS[4]] {
            for human in [Stone::Black, Stone::White] {
                let mut app = test_app();
                app.human_stone = human;
                app.game = opening.game().unwrap();
                app.undo_floor = app.game.history().len();
                if app.game.position().side_to_move() != human {
                    play_first_legal(&mut app.game);
                }
                let decision = app.game.position().clone();
                assert_eq!(app.human_undo_plies(), 0);
                play_first_legal(&mut app.game);
                assert_eq!(app.human_undo_plies(), 1);
                app.undo_to_human();
                assert_eq!(app.game.position(), &decision);
                play_first_legal(&mut app.game);
                play_first_legal(&mut app.game);
                assert_eq!(app.human_undo_plies(), 2);
                app.undo_to_human();
                app.undo_to_human();
                assert_eq!(app.game.position(), &decision);
                assert!(app.game.history().len() >= opening.moves.len());
                assert!(
                    app.game
                        .history()
                        .take(opening.moves.len())
                        .eq(opening.moves.iter().copied())
                );
            }
        }
        for human in [Stone::Black, Stone::White] {
            let mut app = test_app();
            app.human_stone = human;
            app.game = Game::from_record(
                "RustMoku 1\nrules=freestyle\nmoves=D8 A1 E8 C1 F8 E1 G8 G1 H8\n",
            )
            .unwrap();
            app.undo_to_human();
            assert_eq!(app.game.status(), GameStatus::Ongoing);
            assert_eq!(app.game.position().side_to_move(), human);
        }
        let mut white = test_app();
        white.human_stone = Stone::White;
        play_first_legal(&mut white.game); // AI's opening move has no earlier human decision.
        assert_eq!(white.human_undo_plies(), 0);
        white.undo_to_human();
        assert_eq!(white.game.history().len(), 1);
    }

    #[test]
    fn undo_and_import_invalidate_active_requests_and_reject_old_events() {
        let mut app = test_app();
        app.search_limits = SearchLimits::new(20);
        app.play_human_move(Move::CENTER);
        let old = next_event(&app);
        assert!(app.worker.searching());
        app.undo_to_human();
        assert!(!app.worker.searching());
        assert_eq!(app.game, Game::default());
        app.handle_event(old);
        assert_eq!(app.game, Game::default());
        assert!(app.last_search.is_none());
        assert!(app.timings.history.is_empty());

        app.play_human_move(Move::CENTER);
        let position = app.game.position().clone();
        assert!(app.import_record("invalid").is_err());
        assert_eq!(app.game.position(), &position);
        assert!(app.worker.searching());
        // A completed result queued before replacement must not play afterward.
        let old = loop {
            let event = next_event(&app);
            if app.worker.accept(&event) {
                break event;
            }
        };
        let id = match old {
            SearchEvent::Info { id, .. } | SearchEvent::Finished { id, .. } => id,
        };
        let result =
            AlphaBetaEngine::with_config(rustmoku_engine::PatternEvaluator, EngineConfig::new(1))
                .search(&position, SearchLimits::new(1));
        app.undo_floor = 1;
        let record = "RustMoku 1\nrules=freestyle\nmoves=H8 H9 G8 I8\n";
        app.import_record(record).unwrap();
        app.handle_event(SearchEvent::Finished {
            id,
            result,
            elapsed: Duration::ZERO,
        });
        assert_eq!(app.game.to_record(), record);
        assert_eq!(app.undo_floor, 0);
        assert!(!app.worker.searching());
        assert!(app.last_search.is_none());
    }

    #[test]
    fn completed_turn_undo_redo_restores_moves_and_original_timings() {
        let mut app = test_app();
        let human = Move::CENTER;
        let ai = Move::from_row_col(6, 7).unwrap();
        app.game.play_move(human).unwrap();
        app.timings.record_new(Some(Duration::from_millis(1230)));
        app.game.play_move(ai).unwrap();
        app.timings.record_new(Some(Duration::from_millis(4560)));
        let completed = app.game.position().clone();

        app.undo_to_human();
        assert_eq!(app.game.history().len(), 0);
        assert!(app.timings.history.is_empty());
        assert_eq!(
            app.timings.future,
            vec![
                Some(Duration::from_millis(4560)),
                Some(Duration::from_millis(1230))
            ]
        );
        app.redo_to_human();
        assert_eq!(app.game.position(), &completed);
        assert_eq!(
            app.timings.history,
            vec![
                Some(Duration::from_millis(1230)),
                Some(Duration::from_millis(4560))
            ]
        );
        assert!(
            !app.worker.searching(),
            "historical AI reply must not be searched again"
        );
    }

    #[test]
    fn partial_thinking_undo_redo_restores_human_time_and_restarts_ai() {
        let mut app = test_app();
        app.search_limits = SearchLimits::new(1);
        app.game.play_move(Move::CENTER).unwrap();
        app.timings.record_new(Some(Duration::from_millis(875)));
        app.undo_to_human();
        assert!(!app.worker.searching());
        app.redo_to_human();
        assert_eq!(app.game.history().collect::<Vec<_>>(), [Move::CENTER]);
        assert_eq!(app.timings.history, [Some(Duration::from_millis(875))]);
        assert!(app.worker.searching(), "no historical AI reply remains");
    }

    #[test]
    fn timing_metadata_follows_branch_success_and_failure() {
        let mut app = test_app();
        let first = Move::CENTER;
        let reply = Move::from_row_col(6, 7).unwrap();
        let second_human = Move::from_row_col(7, 6).unwrap();
        let second_reply = Move::from_row_col(6, 6).unwrap();
        app.game.play_move(first).unwrap();
        app.timings.record_new(Some(Duration::from_secs(1)));
        app.game.play_move(reply).unwrap();
        app.timings.record_new(Some(Duration::from_secs(2)));
        app.game.play_move(second_human).unwrap();
        app.timings.record_new(Some(Duration::from_secs(3)));
        app.game.play_move(second_reply).unwrap();
        app.timings.record_new(Some(Duration::from_secs(4)));
        app.undo_to_human();
        let future = app.timings.future.clone();
        app.play_human_move(first);
        assert_eq!(
            app.timings.future, future,
            "illegal move preserves timing future"
        );
        let branch = Move::from_row_col(7, 8).unwrap();
        app.play_human_move(branch);
        assert!(app.timings.future.is_empty());
        assert_eq!(app.game.redo_len(), 0);
        assert!(!app.game.to_record().contains("time"));
    }

    #[test]
    fn current_cancelled_result_cannot_record_ai_timing() {
        let mut app = test_app();
        app.search_limits = SearchLimits::new(1);
        app.play_human_move(Move::CENTER);
        let event = next_event(&app);
        let id = match event {
            SearchEvent::Info { id, .. } | SearchEvent::Finished { id, .. } => id,
        };
        let mut result =
            AlphaBetaEngine::with_config(rustmoku_engine::PatternEvaluator, EngineConfig::new(1))
                .search(app.game.position(), SearchLimits::new(1));
        result.termination = SearchTermination::Cancelled;
        app.handle_event(SearchEvent::Finished {
            id,
            result,
            elapsed: Duration::from_secs(99),
        });
        assert_eq!(app.game.history().len(), 1);
        assert_eq!(app.timings.history.len(), 1);
    }

    #[test]
    fn imported_and_opening_moves_have_unknown_session_timing() {
        let mut app = test_app();
        app.import_record("RustMoku 1\nrules=freestyle\nmoves=H8 H9 G8 I8\n")
            .unwrap();
        assert_eq!(app.timings.history, [None; 4]);
        app.selected_opening = Some(0);
        app.new_game();
        assert_eq!(app.timings.history, vec![None; OPENINGS[0].moves.len()]);
    }

    fn next_event(app: &RustMokuApp) -> SearchEvent {
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match app.worker.poll() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) if std::time::Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                other => panic!("worker did not produce an event: {:?}", other.err()),
            }
        }
    }

    #[test]
    fn stale_info_and_result_cannot_modify_new_game_or_duplicate_ai_move() {
        let mut app = RustMokuApp::with_worker(SearchWorker::new(EngineConfig::new(1)).unwrap());
        app.human_stone = Stone::White;
        app.search_limits = SearchLimits::new(1);
        app.play_ai_if_needed();
        let old = next_event(&app);
        let old_id = match &old {
            SearchEvent::Info { id, .. } | SearchEvent::Finished { id, .. } => *id,
        };
        let result =
            AlphaBetaEngine::with_config(rustmoku_engine::PatternEvaluator, EngineConfig::new(1))
                .search(app.game.position(), SearchLimits::new(1));
        app.new_game();
        app.handle_event(old);
        app.handle_event(SearchEvent::Info {
            id: old_id,
            info: SearchInfo::from(&result),
        });
        app.handle_event(SearchEvent::Finished {
            id: old_id,
            result: result.clone(),
            elapsed: Duration::ZERO,
        });
        assert_eq!(app.game.position().move_count(), 0);
        assert!(app.last_search.is_none());
        assert!(app.worker.searching());
        let mut current_id = old_id;
        while app.worker.searching() {
            let event = next_event(&app);
            if let SearchEvent::Finished { id, .. } = &event {
                current_id = *id;
            }
            app.handle_event(event);
        }
        assert_eq!(app.game.position().move_count(), 1);
        app.handle_event(SearchEvent::Finished {
            id: current_id,
            result,
            elapsed: Duration::ZERO,
        });
        assert_eq!(app.game.position().move_count(), 1);
    }
}
