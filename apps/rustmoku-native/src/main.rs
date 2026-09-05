#![forbid(unsafe_code)]

use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use rustmoku_core::{BOARD_SIZE, Game, GameStatus, Move, MoveError, OPENINGS, RecordError, Stone};
use rustmoku_engine::{EngineConfig, SearchInfo, SearchLimits, SearchTermination};
use std::{sync::mpsc::TryRecvError, time::Duration};
mod worker;
use worker::{SearchEvent, SearchWorker};

const BOARD_MARGIN: f32 = 28.0;
const BOARD_COLOR: Color32 = Color32::from_rgb(216, 171, 103);
const GRID_COLOR: Color32 = Color32::from_rgb(63, 45, 28);
const LAST_MOVE_COLOR: Color32 = Color32::from_rgb(210, 48, 42);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 850.0])
            .with_min_inner_size([720.0, 650.0]),
        ..Default::default()
    };

    eframe::run_native(
        "RustMoku V0.8",
        options,
        Box::new(|_creation_context| Ok(Box::new(RustMokuApp::new()?))),
    )
}

struct RustMokuApp {
    game: Game,
    human_stone: Stone,
    worker: SearchWorker,
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
}

impl RustMokuApp {
    fn new() -> std::io::Result<Self> {
        Ok(Self::with_worker(SearchWorker::new(
            EngineConfig::default(),
        )?))
    }

    fn with_worker(worker: SearchWorker) -> Self {
        Self {
            game: Game::default(),
            human_stone: Stone::Black,
            worker,
            move_time_ms: 0,
            search_limits: SearchLimits::default(),
            last_search: None,
            message: None,
            selected_opening: None,
            undo_floor: 0,
            show_move_numbers: false,
            record_open: false,
            record_text: String::new(),
            record_path: String::from("rustmoku-game.rmk"),
        }
    }

    fn replace_game(&mut self, game: Game, undo_floor: usize) {
        self.worker.invalidate();
        self.game = game;
        self.undo_floor = undo_floor;
        self.last_search = None;
        self.message = None;
        self.play_ai_if_needed();
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
        self.game.undo_plies(plies);
        self.last_search = None;
        self.message = None;
        self.play_ai_if_needed();
    }

    fn play_human_move(&mut self, at: Move) {
        if self.game.status() != GameStatus::Ongoing
            || self.game.position().side_to_move() != self.human_stone
        {
            return;
        }

        match self.game.play_move(at) {
            Ok(()) => {
                self.worker.invalidate();
                self.message = None;
                self.play_ai_if_needed();
            }
            Err(MoveError::Occupied { .. }) => {
                self.message = Some(String::from("That intersection is occupied."));
            }
            Err(error) => {
                self.message = Some(error.to_string());
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
            self.message = Some(error.into());
        }
    }

    fn handle_event(&mut self, event: SearchEvent) {
        if !self.worker.accept(&event) {
            return;
        }
        match event {
            SearchEvent::Info { info, .. } => self.last_search = Some(info),
            SearchEvent::Finished { result, .. } => {
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
                    self.message = Some("The engine found no legal move.".into());
                    return;
                };
                if let Err(error) = self.game.play_move(at) {
                    self.message = Some(format!("Engine move failed: {error}"));
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
                        self.message = Some("Search worker disconnected.".into());
                    }
                    break;
                }
            }
        }
    }

    fn status_text(&self) -> String {
        match self.game.status() {
            GameStatus::Won(stone) if stone == self.human_stone => String::from("You win!"),
            GameStatus::Won(_) => String::from("AI wins."),
            GameStatus::Draw => String::from("Draw."),
            GameStatus::Ongoing if self.game.position().side_to_move() == self.human_stone => {
                format!("Your turn ({})", stone_name(self.human_stone))
            }
            GameStatus::Ongoing => {
                format!("AI searching ({})", stone_name(self.human_stone.opponent()))
            }
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("RustMoku V0.8");
        ui.horizontal(|ui| {
            ui.label("Play as:");
            let previous_stone = self.human_stone;
            ui.radio_value(&mut self.human_stone, Stone::Black, "Black");
            ui.radio_value(&mut self.human_stone, Stone::White, "White");
            if self.human_stone != previous_stone {
                self.new_game();
            }
            if ui.button("New Game").clicked() {
                self.new_game();
            }
            if ui
                .add_enabled(self.human_undo_plies() > 0, egui::Button::new("Undo turn"))
                .clicked()
            {
                self.undo_to_human();
            }
            if ui.button("Game record...").clicked() {
                self.record_text = self.game.to_record();
                self.record_open = true;
            }
            ui.checkbox(&mut self.show_move_numbers, "Move numbers");
        });
        ui.horizontal(|ui| {
            ui.label("Opening:");
            let previous = self.selected_opening;
            egui::ComboBox::from_id_salt("opening")
                .selected_text(
                    self.selected_opening
                        .map_or("Empty Board", |i| OPENINGS[i].name),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.selected_opening, None, "Empty Board");
                    for (index, opening) in OPENINGS.iter().enumerate() {
                        ui.selectable_value(&mut self.selected_opening, Some(index), opening.name);
                    }
                });
            if ui.button("Next in suite").clicked() {
                self.selected_opening = Some(
                    self.selected_opening
                        .map_or(0, |i| (i + 1) % OPENINGS.len()),
                );
            }
            if self.selected_opening != previous {
                self.new_game();
            }
            ui.small("Hand-authored test starts; no balance claim");
        });
        ui.horizontal(|ui| {
            ui.label("Next depth:");
            ui.add(egui::DragValue::new(&mut self.search_limits.max_depth).range(1..=12));
            ui.label("Move ms (0 = unlimited):");
            ui.add(egui::DragValue::new(&mut self.move_time_ms).range(0..=60_000));
            if self.worker.searching() {
                ui.spinner();
            }
            if let Some(at) = self.game.position().last_move() {
                ui.label(format!("Last: {at}"));
            }
        });
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
                egui::Label::new(format!(
                    "AI: depth {} | seldepth {} | work {} (q {}) | score {}",
                    search.completed_depth,
                    search.seldepth,
                    search.statistics.work_nodes,
                    search.statistics.qnodes,
                    search.score
                ))
                .truncate(),
            );
            ui.add(
                egui::Label::new(format!(
                    "TT: hits {} | cutoffs {} | probes {} | stores {}",
                    search.statistics.tt_hits,
                    search.statistics.tt_cutoffs,
                    search.statistics.tt_probes,
                    search.statistics.tt_stores
                ))
                .truncate(),
            );
            ui.label(search.tactical_proof.map_or_else(
                || String::from("No exact tactical proof"),
                |proof| format!("{:?} proven, {} plies", proof.kind, proof.plies),
            ));
        } else {
            ui.label("AI: waiting for a completed search depth");
            ui.label("TT: -");
            ui.label("No exact tactical proof");
        }
        egui::ScrollArea::horizontal()
            .id_salt("pv")
            .max_height(28.0)
            .min_scrolled_height(28.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.monospace("PV:");
                    if let Some(search) = &self.last_search {
                        for at in &search.principal_variation {
                            ui.monospace(at.to_string());
                        }
                    }
                });
            });
    }

    fn move_history(&self, ui: &mut egui::Ui) {
        ui.heading("Moves");
        ui.small(format!("Undo floor: {} plies", self.undo_floor));
        egui::ScrollArea::vertical()
            .id_salt("history")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("move_list").striped(true).show(ui, |ui| {
                    ui.label("#");
                    ui.label("Black");
                    ui.label("White");
                    ui.end_row();
                    let mut moves = self.game.history();
                    let mut turn = 1;
                    while let Some(black) = moves.next() {
                        ui.label(turn.to_string());
                        ui.monospace(black.to_string());
                        ui.monospace(
                            moves
                                .next()
                                .map_or_else(String::new, |white| white.to_string()),
                        );
                        ui.end_row();
                        turn += 1;
                    }
                });
            });
    }

    fn record_window(&mut self, ctx: &egui::Context) {
        if !self.record_open {
            return;
        }
        let mut open = self.record_open;
        egui::Window::new("Game record")
            .open(&mut open)
            .default_width(590.0)
            .show(ctx, |ui| {
                ui.label(
                    "Export the complete played sequence, or paste a RustMoku record to import.",
                );
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
                    if ui.button("Export current game").clicked() {
                        self.record_text = self.game.to_record();
                    }
                    if ui.button("Copy text").clicked() {
                        ctx.copy_text(self.record_text.clone());
                    }
                    if ui.button("Import text").clicked() {
                        let text = self.record_text.clone();
                        if let Err(error) = self.import_record(&text) {
                            self.message = Some(error.to_string());
                        } else {
                            self.message = Some("Record imported.".into());
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("File:");
                    ui.text_edit_singleline(&mut self.record_path);
                });
                ui.horizontal(|ui| {
                    if ui.button("Load file into editor").clicked() {
                        match std::fs::read_to_string(&self.record_path) {
                            Ok(text) => {
                                self.record_text = text;
                                self.message = None;
                            }
                            Err(error) => self.message = Some(format!("Load failed: {error}")),
                        }
                    }
                    if ui.button("Save editor to file (replace)").clicked() {
                        // Validate before writing; output is canonical record syntax.
                        match Game::from_record(&self.record_text) {
                            Ok(game) => match std::fs::write(&self.record_path, game.to_record()) {
                                Ok(()) => self.message = Some("Record saved.".into()),
                                Err(error) => self.message = Some(format!("Save failed: {error}")),
                            },
                            Err(error) => self.message = Some(error.to_string()),
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
        if self.worker.searching() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        // Stable panel sizes isolate board geometry from PV/proof/history text.
        egui::Panel::top("controls")
            .exact_size(238.0)
            .resizable(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("controls_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.controls(ui));
            });
        egui::Panel::right("history_panel")
            .exact_size(180.0)
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

const fn stone_name(stone: Stone) -> &'static str {
    match stone {
        Stone::Black => "Black",
        Stone::White => "White",
    }
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
        app.handle_event(SearchEvent::Finished { id, result });
        assert_eq!(app.game.to_record(), record);
        assert_eq!(app.undo_floor, 0);
        assert!(!app.worker.searching());
        assert!(app.last_search.is_none());
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
        });
        assert_eq!(app.game.position().move_count(), 1);
    }
}
