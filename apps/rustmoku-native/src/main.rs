#![forbid(unsafe_code)]

use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use rustmoku_core::{BOARD_SIZE, Game, GameStatus, Move, MoveError, Stone};
use rustmoku_engine::{AlphaBetaEngine, SearchEngine, SearchLimits, SearchResult};

const BOARD_MARGIN: f32 = 28.0;
const BOARD_COLOR: Color32 = Color32::from_rgb(216, 171, 103);
const GRID_COLOR: Color32 = Color32::from_rgb(63, 45, 28);
const LAST_MOVE_COLOR: Color32 = Color32::from_rgb(210, 48, 42);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 820.0])
            .with_min_inner_size([520.0, 620.0]),
        ..Default::default()
    };

    eframe::run_native(
        "RustMoku V0.2",
        options,
        Box::new(|_creation_context| Ok(Box::new(RustMokuApp::default()))),
    )
}

struct RustMokuApp {
    game: Game,
    human_stone: Stone,
    engine: AlphaBetaEngine,
    search_limits: SearchLimits,
    last_search: Option<SearchResult>,
    message: Option<String>,
}

impl Default for RustMokuApp {
    fn default() -> Self {
        Self {
            game: Game::default(),
            human_stone: Stone::Black,
            engine: AlphaBetaEngine::default(),
            search_limits: SearchLimits::default(),
            last_search: None,
            message: None,
        }
    }
}

impl RustMokuApp {
    fn new_game(&mut self) {
        self.game = Game::default();
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
        if self.game.status() != GameStatus::Ongoing
            || self.game.position().side_to_move() == self.human_stone
        {
            return;
        }

        let result = self.engine.search(self.game.position(), self.search_limits);
        let best_move = result.best_move;
        self.last_search = Some(result);
        let Some(best_move) = best_move else {
            self.message = Some(String::from("The engine found no legal move."));
            return;
        };
        if let Err(error) = self.game.play_move(best_move) {
            self.message = Some(format!("Engine move failed: {error}"));
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
                format!("AI to move ({})", stone_name(self.human_stone.opponent()))
            }
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("RustMoku V0.2");
        ui.add_space(4.0);
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
        });
        ui.horizontal(|ui| {
            ui.strong(self.status_text());
            if let Some(message) = &self.message {
                ui.colored_label(Color32::from_rgb(190, 45, 40), message);
            }
        });
        if let Some(search) = &self.last_search {
            ui.label(format!(
                "AI search: depth {}/{}  |  seldepth {}  |  nodes {}  |  score {}",
                search.completed_depth,
                search.requested_depth,
                search.seldepth,
                search.statistics.nodes,
                search.score
            ));
            ui.label(format!(
                "TT: hits {}  |  cutoffs {}  |  probes {}  |  stores {}",
                search.statistics.tt_hits,
                search.statistics.tt_cutoffs,
                search.statistics.tt_probes,
                search.statistics.tt_stores,
            ));
            ui.horizontal_wrapped(|ui| {
                ui.label("PV:");
                if search.principal_variation.is_empty() {
                    ui.monospace("(empty)");
                } else {
                    for at in &search.principal_variation {
                        ui.monospace(format!("{}({},{})", at.index(), at.row(), at.column()));
                    }
                }
            });
        } else {
            ui.label("AI search: no search yet");
        }
    }

    fn board(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let side = available.x.min(available.y).max(300.0);
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
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(8.0);
            self.controls(ui);
            ui.separator();
            ui.add_space(8.0);
            ui.vertical_centered(|ui| self.board(ui));
        });
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
