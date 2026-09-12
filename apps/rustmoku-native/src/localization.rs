use std::{ffi::OsString, path::PathBuf, sync::Arc};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use rustmoku_core::{Move, Stone};
use rustmoku_engine::{ProofDistance, SearchInfo, SearchOrigin};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LanguagePreference {
    Auto,
    English,
    SimplifiedChinese,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UiLanguage {
    English,
    SimplifiedChinese,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TextKey {
    Language,
    Auto,
    English,
    SimplifiedChinese,
    PlayAs,
    Black,
    White,
    NewGame,
    UndoTurn,
    RedoTurn,
    GameRecord,
    MoveNumbers,
    Opening,
    EmptyBoard,
    NextInSuite,
    OpeningNote,
    NextDepth,
    MoveTime,
    UnlimitedHint,
    Last,
    Threads,
    Manual,
    TtPrimary,
    Searching,
    Evaluator,
    PatternEval,
    ModelPath,
    SearchProfile,
    LoadProfile,
    ResearchProfile,
    BaselineProfile,
    LoadModel,
    UsePattern,
    CurrentMove,
    Time,
    Waiting,
    Moves,
    RecordHelp,
    ExportGame,
    CopyText,
    ImportText,
    File,
    LoadFile,
    SaveFile,
    RecordImported,
    RecordSaved,
    Occupied,
    NoLegalMove,
    WorkerDisconnected,
    CjkFontMissing,
    MoveFailed,
    SearchFailed,
    EngineMoveFailed,
    ReconfigureFailed,
    ImportFailed,
    LoadFailed,
    SaveFailed,
    InvalidRecord,
    TtEmpty,
    Pv,
}

#[derive(Clone, Copy)]
pub(super) struct UiText {
    language: UiLanguage,
}

impl LanguagePreference {
    pub(super) fn resolve(self, locale: Option<&str>) -> UiLanguage {
        match self {
            Self::Auto if locale.is_some_and(is_simplified_chinese_locale) => {
                UiLanguage::SimplifiedChinese
            }
            Self::Auto | Self::English => UiLanguage::English,
            Self::SimplifiedChinese => UiLanguage::SimplifiedChinese,
        }
    }
}

impl UiText {
    pub(super) const fn new(language: UiLanguage) -> Self {
        Self { language }
    }

    pub(super) const fn get(self, key: TextKey) -> &'static str {
        match (self.language, key) {
            (UiLanguage::English, TextKey::Language) => "Language:",
            (UiLanguage::English, TextKey::Auto) => "Auto",
            (UiLanguage::English, TextKey::English) => "English",
            (UiLanguage::English, TextKey::SimplifiedChinese) => "Simplified Chinese",
            (UiLanguage::English, TextKey::PlayAs) => "Play as:",
            (UiLanguage::English, TextKey::Black) => "Black",
            (UiLanguage::English, TextKey::White) => "White",
            (UiLanguage::English, TextKey::NewGame) => "New Game",
            (UiLanguage::English, TextKey::UndoTurn) => "Undo turn",
            (UiLanguage::English, TextKey::RedoTurn) => "Redo turn",
            (UiLanguage::English, TextKey::GameRecord) => "Game record...",
            (UiLanguage::English, TextKey::MoveNumbers) => "Move numbers",
            (UiLanguage::English, TextKey::Opening) => "Opening:",
            (UiLanguage::English, TextKey::EmptyBoard) => "Empty Board",
            (UiLanguage::English, TextKey::NextInSuite) => "Next in suite",
            (UiLanguage::English, TextKey::OpeningNote) => {
                "Hand-authored test starts; no balance claim"
            }
            (UiLanguage::English, TextKey::NextDepth) => "Next depth:",
            (UiLanguage::English, TextKey::MoveTime) => "Move ms:",
            (UiLanguage::English, TextKey::UnlimitedHint) => "0 = unlimited",
            (UiLanguage::English, TextKey::Last) => "Last",
            (UiLanguage::English, TextKey::Threads) => "Threads:",
            (UiLanguage::English, TextKey::Manual) => "Manual",
            (UiLanguage::English, TextKey::TtPrimary) => "TT primary MiB:",
            (UiLanguage::English, TextKey::Searching) => "Searching...",
            (UiLanguage::English, TextKey::Evaluator) => "Evaluator:",
            (UiLanguage::English, TextKey::PatternEval) => "Pattern",
            (UiLanguage::English, TextKey::SearchProfile) => "Search profile:",
            (UiLanguage::English, TextKey::LoadProfile) => "Load profile",
            (UiLanguage::English, TextKey::ResearchProfile) => "Research options active",
            (UiLanguage::English, TextKey::BaselineProfile) => "Baseline research options",
            (UiLanguage::English, TextKey::ModelPath) => "Model:",
            (UiLanguage::English, TextKey::LoadModel) => "Load model",
            (UiLanguage::English, TextKey::UsePattern) => "Use Pattern",
            (UiLanguage::English, TextKey::CurrentMove) => "Current move",
            (UiLanguage::English, TextKey::Time) => "Time",
            (UiLanguage::English, TextKey::Waiting) => "AI: waiting for a completed search depth",
            (UiLanguage::English, TextKey::Moves) => "Moves",
            (UiLanguage::English, TextKey::RecordHelp) => {
                "Export the complete played sequence, or paste a RustMoku record to import."
            }
            (UiLanguage::English, TextKey::ExportGame) => "Export current game",
            (UiLanguage::English, TextKey::CopyText) => "Copy text",
            (UiLanguage::English, TextKey::ImportText) => "Import text",
            (UiLanguage::English, TextKey::File) => "File:",
            (UiLanguage::English, TextKey::LoadFile) => "Load file into editor",
            (UiLanguage::English, TextKey::SaveFile) => "Save editor to file (replace)",
            (UiLanguage::English, TextKey::RecordImported) => "Record imported.",
            (UiLanguage::English, TextKey::RecordSaved) => "Record saved.",
            (UiLanguage::English, TextKey::Occupied) => "That intersection is occupied.",
            (UiLanguage::English, TextKey::NoLegalMove) => "The engine found no legal move.",
            (UiLanguage::English, TextKey::WorkerDisconnected) => "Search worker disconnected.",
            (UiLanguage::English, TextKey::CjkFontMissing) => {
                "Chinese font unavailable; using English."
            }
            (UiLanguage::English, TextKey::MoveFailed) => "Move failed",
            (UiLanguage::English, TextKey::SearchFailed) => "Search failed",
            (UiLanguage::English, TextKey::EngineMoveFailed) => "Engine move failed",
            (UiLanguage::English, TextKey::ReconfigureFailed) => "Reconfigure failed",
            (UiLanguage::English, TextKey::ImportFailed) => "Import failed",
            (UiLanguage::English, TextKey::LoadFailed) => "Load failed",
            (UiLanguage::English, TextKey::SaveFailed) => "Save failed",
            (UiLanguage::English, TextKey::InvalidRecord) => "Invalid record",
            (UiLanguage::English, TextKey::TtEmpty) => "TT: -",
            (UiLanguage::English, TextKey::Pv) => "PV:",
            (UiLanguage::SimplifiedChinese, TextKey::Language) => "语言：",
            (UiLanguage::SimplifiedChinese, TextKey::Auto) => "自动",
            (UiLanguage::SimplifiedChinese, TextKey::English) => "English",
            (UiLanguage::SimplifiedChinese, TextKey::SimplifiedChinese) => "简体中文",
            (UiLanguage::SimplifiedChinese, TextKey::PlayAs) => "执子：",
            (UiLanguage::SimplifiedChinese, TextKey::Black) => "黑棋",
            (UiLanguage::SimplifiedChinese, TextKey::White) => "白棋",
            (UiLanguage::SimplifiedChinese, TextKey::NewGame) => "新对局",
            (UiLanguage::SimplifiedChinese, TextKey::UndoTurn) => "悔棋",
            (UiLanguage::SimplifiedChinese, TextKey::RedoTurn) => "重做",
            (UiLanguage::SimplifiedChinese, TextKey::GameRecord) => "棋谱...",
            (UiLanguage::SimplifiedChinese, TextKey::MoveNumbers) => "显示手数",
            (UiLanguage::SimplifiedChinese, TextKey::Opening) => "开局：",
            (UiLanguage::SimplifiedChinese, TextKey::EmptyBoard) => "空棋盘",
            (UiLanguage::SimplifiedChinese, TextKey::NextInSuite) => "下一开局",
            (UiLanguage::SimplifiedChinese, TextKey::OpeningNote) => "手工测试开局；不代表平衡性",
            (UiLanguage::SimplifiedChinese, TextKey::NextDepth) => "搜索深度：",
            (UiLanguage::SimplifiedChinese, TextKey::MoveTime) => "每步毫秒：",
            (UiLanguage::SimplifiedChinese, TextKey::UnlimitedHint) => "0 = 不限时",
            (UiLanguage::SimplifiedChinese, TextKey::Last) => "上一手",
            (UiLanguage::SimplifiedChinese, TextKey::Threads) => "线程：",
            (UiLanguage::SimplifiedChinese, TextKey::Manual) => "手动",
            (UiLanguage::SimplifiedChinese, TextKey::TtPrimary) => "置换表主容量 MiB：",
            (UiLanguage::SimplifiedChinese, TextKey::Searching) => "搜索中……",
            (UiLanguage::SimplifiedChinese, TextKey::Evaluator) => "评估器：",
            (UiLanguage::SimplifiedChinese, TextKey::PatternEval) => "模式评估",
            (UiLanguage::SimplifiedChinese, TextKey::SearchProfile) => "搜索配置：",
            (UiLanguage::SimplifiedChinese, TextKey::LoadProfile) => "载入配置",
            (UiLanguage::SimplifiedChinese, TextKey::ResearchProfile) => "研究选项已启用",
            (UiLanguage::SimplifiedChinese, TextKey::BaselineProfile) => "基线研究选项",
            (UiLanguage::SimplifiedChinese, TextKey::ModelPath) => "模型：",
            (UiLanguage::SimplifiedChinese, TextKey::LoadModel) => "载入模型",
            (UiLanguage::SimplifiedChinese, TextKey::UsePattern) => "使用模式评估",
            (UiLanguage::SimplifiedChinese, TextKey::CurrentMove) => "本手用时",
            (UiLanguage::SimplifiedChinese, TextKey::Time) => "用时",
            (UiLanguage::SimplifiedChinese, TextKey::Waiting) => "AI：等待完成搜索深度",
            (UiLanguage::SimplifiedChinese, TextKey::Moves) => "着法",
            (UiLanguage::SimplifiedChinese, TextKey::RecordHelp) => {
                "导出完整对局，或粘贴 RustMoku 棋谱后导入。"
            }
            (UiLanguage::SimplifiedChinese, TextKey::ExportGame) => "导出当前对局",
            (UiLanguage::SimplifiedChinese, TextKey::CopyText) => "复制文本",
            (UiLanguage::SimplifiedChinese, TextKey::ImportText) => "导入文本",
            (UiLanguage::SimplifiedChinese, TextKey::File) => "文件：",
            (UiLanguage::SimplifiedChinese, TextKey::LoadFile) => "载入文件到编辑器",
            (UiLanguage::SimplifiedChinese, TextKey::SaveFile) => "保存编辑器内容（覆盖）",
            (UiLanguage::SimplifiedChinese, TextKey::RecordImported) => "棋谱已导入。",
            (UiLanguage::SimplifiedChinese, TextKey::RecordSaved) => "棋谱已保存。",
            (UiLanguage::SimplifiedChinese, TextKey::Occupied) => "该交叉点已有棋子。",
            (UiLanguage::SimplifiedChinese, TextKey::NoLegalMove) => "引擎未找到合法着法。",
            (UiLanguage::SimplifiedChinese, TextKey::WorkerDisconnected) => "搜索线程已断开。",
            (UiLanguage::SimplifiedChinese, TextKey::CjkFontMissing) => {
                "中文字体不可用；已切换为英文。"
            }
            (UiLanguage::SimplifiedChinese, TextKey::MoveFailed) => "着法失败",
            (UiLanguage::SimplifiedChinese, TextKey::SearchFailed) => "搜索失败",
            (UiLanguage::SimplifiedChinese, TextKey::EngineMoveFailed) => "引擎着法失败",
            (UiLanguage::SimplifiedChinese, TextKey::ReconfigureFailed) => "重新配置失败",
            (UiLanguage::SimplifiedChinese, TextKey::ImportFailed) => "导入失败",
            (UiLanguage::SimplifiedChinese, TextKey::LoadFailed) => "载入失败",
            (UiLanguage::SimplifiedChinese, TextKey::SaveFailed) => "保存失败",
            (UiLanguage::SimplifiedChinese, TextKey::InvalidRecord) => "棋谱无效",
            (UiLanguage::SimplifiedChinese, TextKey::TtEmpty) => "置换表：-",
            (UiLanguage::SimplifiedChinese, TextKey::Pv) => "主变：",
        }
    }

    pub(super) const fn stone(self, stone: Stone) -> &'static str {
        self.get(match stone {
            Stone::Black => TextKey::Black,
            Stone::White => TextKey::White,
        })
    }

    pub(super) fn detail(self, prefix: TextKey, detail: &str) -> String {
        let separator = match self.language {
            UiLanguage::English => ": ",
            UiLanguage::SimplifiedChinese => "：",
        };
        format!("{}{separator}{detail}", self.get(prefix))
    }

    pub(super) fn status(
        self,
        won: Option<Stone>,
        draw: bool,
        human: Stone,
        side_to_move: Stone,
    ) -> String {
        match self.language {
            UiLanguage::English => match (won, draw, side_to_move == human) {
                (Some(stone), _, _) if stone == human => "You win!".into(),
                (Some(_), _, _) => "AI wins.".into(),
                (None, true, _) => "Draw.".into(),
                (None, false, true) => format!("Your turn ({})", self.stone(human)),
                (None, false, false) => format!("AI searching ({})", self.stone(human.opponent())),
            },
            UiLanguage::SimplifiedChinese => match (won, draw, side_to_move == human) {
                (Some(stone), _, _) if stone == human => "你赢了！".into(),
                (Some(_), _, _) => "AI 获胜。".into(),
                (None, true, _) => "和棋。".into(),
                (None, false, true) => format!("轮到你（{}）", self.stone(human)),
                (None, false, false) => format!("AI 搜索中（{}）", self.stone(human.opponent())),
            },
        }
    }

    pub(super) fn search_summary(
        self,
        depth: u8,
        seldepth: u8,
        work: u64,
        qnodes: u64,
        threads: usize,
        score: i32,
    ) -> String {
        match self.language {
            UiLanguage::English => format!(
                "AI: depth {depth} | seldepth {seldepth} | work {work} (q {qnodes}) | threads {threads} | score {score}"
            ),
            UiLanguage::SimplifiedChinese => format!(
                "AI：深度 {depth} | 选择深度 {seldepth} | 工作量 {work}（静态搜索节点 {qnodes}）| 线程 {threads} | 分数 {score}"
            ),
        }
    }

    pub(super) fn tt_summary(self, hits: u64, cutoffs: u64, probes: u64, stores: u64) -> String {
        match self.language {
            UiLanguage::English => {
                format!("TT: hits {hits} | cutoffs {cutoffs} | probes {probes} | stores {stores}")
            }
            UiLanguage::SimplifiedChinese => {
                format!("置换表：命中 {hits} | 截断 {cutoffs} | 探测 {probes} | 存储 {stores}")
            }
        }
    }

    pub(super) fn result_summary(self, search: &SearchInfo) -> String {
        let proof_distance = search
            .proof
            .map(|proof| match (self.language, proof.distance) {
                (UiLanguage::English, ProofDistance::Exact(plies)) => {
                    format!("exact {plies} plies")
                }
                (UiLanguage::English, ProofDistance::AtMost(plies)) => {
                    format!("within {plies} plies")
                }
                (UiLanguage::SimplifiedChinese, ProofDistance::Exact(plies)) => {
                    format!("精确 {plies} 层")
                }
                (UiLanguage::SimplifiedChinese, ProofDistance::AtMost(plies)) => {
                    format!("最多 {plies} 层")
                }
            });
        match (self.language, search.origin) {
            (UiLanguage::English, SearchOrigin::Analysis) => "Analysis only".into(),
            (UiLanguage::SimplifiedChinese, SearchOrigin::Analysis) => "仅分析".into(),
            (UiLanguage::English, SearchOrigin::Fallback) => "Result: fallback".into(),
            (UiLanguage::SimplifiedChinese, SearchOrigin::Fallback) => "结果来源：回退结果".into(),
            (UiLanguage::English, SearchOrigin::AlphaBeta) => "Result: Alpha-Beta".into(),
            (UiLanguage::SimplifiedChinese, SearchOrigin::AlphaBeta) => {
                "结果来源：Alpha-Beta".into()
            }
            (UiLanguage::English, SearchOrigin::Terminal) => "Result: terminal position".into(),
            (UiLanguage::SimplifiedChinese, SearchOrigin::Terminal) => "结果来源：终局".into(),
            (UiLanguage::English, SearchOrigin::Immediate) => {
                "Result: exact immediate tactic".into()
            }
            (UiLanguage::SimplifiedChinese, SearchOrigin::Immediate) => {
                "结果来源：即时精确战术".into()
            }
            (UiLanguage::English, SearchOrigin::Vcf) => format!(
                "Proof: VCF · {}",
                proof_distance.unwrap_or_else(|| "verified".into())
            ),
            (UiLanguage::SimplifiedChinese, SearchOrigin::Vcf) => format!(
                "证明：VCF · {}",
                proof_distance.unwrap_or_else(|| "已验证".into())
            ),
            (UiLanguage::English, SearchOrigin::Vct) => format!(
                "Proof: VCT/DFPN · {}",
                proof_distance.unwrap_or_else(|| "verified".into())
            ),
            (UiLanguage::SimplifiedChinese, SearchOrigin::Vct) => format!(
                "证明：VCT/DFPN · {}",
                proof_distance.unwrap_or_else(|| "已验证".into())
            ),
            (UiLanguage::English, SearchOrigin::ProofBook) => format!(
                "Proof: Verified Proof Book · {}",
                proof_distance.unwrap_or_else(|| "verified".into())
            ),
            (UiLanguage::SimplifiedChinese, SearchOrigin::ProofBook) => format!(
                "证明：已验证证明库 · {}",
                proof_distance.unwrap_or_else(|| "已验证".into())
            ),
        }
    }

    pub(super) fn current_move_time(self, elapsed: Duration) -> String {
        match self.language {
            UiLanguage::English => format!(
                "{}: {:.1} s",
                self.get(TextKey::CurrentMove),
                elapsed.as_secs_f64()
            ),
            UiLanguage::SimplifiedChinese => format!(
                "{}：{:.1} 秒",
                self.get(TextKey::CurrentMove),
                elapsed.as_secs_f64()
            ),
        }
    }

    pub(super) fn last_move(self, at: Move, elapsed: Option<Duration>) -> String {
        match (self.language, elapsed) {
            (UiLanguage::English, Some(elapsed)) => {
                format!(
                    "{}: {at} · {:.2} s",
                    self.get(TextKey::Last),
                    elapsed.as_secs_f64()
                )
            }
            (UiLanguage::SimplifiedChinese, Some(elapsed)) => {
                format!(
                    "{}：{at} · {:.2} 秒",
                    self.get(TextKey::Last),
                    elapsed.as_secs_f64()
                )
            }
            (UiLanguage::English, None) => format!("{}: {at}", self.get(TextKey::Last)),
            (UiLanguage::SimplifiedChinese, None) => format!("{}：{at}", self.get(TextKey::Last)),
        }
    }

    pub(super) fn history_time(self, elapsed: Option<Duration>) -> String {
        match (self.language, elapsed) {
            (UiLanguage::English, Some(elapsed)) => format!("{:.2}s", elapsed.as_secs_f64()),
            (UiLanguage::SimplifiedChinese, Some(elapsed)) => {
                format!("{:.2}秒", elapsed.as_secs_f64())
            }
            (_, None) => "—".into(),
        }
    }

    pub(super) fn undo_floor(self, plies: usize) -> String {
        match self.language {
            UiLanguage::English => format!("Undo floor: {plies} plies"),
            UiLanguage::SimplifiedChinese => format!("悔棋下限：{plies} 层"),
        }
    }
}

pub(super) fn is_simplified_chinese_locale(locale: &str) -> bool {
    let locale = locale.to_ascii_lowercase().replace('_', "-");
    locale == "zh"
        || locale.starts_with("zh-cn")
        || locale.starts_with("zh-sg")
        || locale.starts_with("zh-hans")
}

pub(super) fn install_windows_cjk_font(ctx: &egui::Context) -> bool {
    let windows = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .unwrap_or_else(|| OsString::from(r"C:\Windows"));
    let fonts = PathBuf::from(windows).join("Fonts");
    let bytes = ["msyh.ttc", "msyh.ttf", "simhei.ttf", "simsun.ttc"]
        .into_iter()
        .find_map(|name| std::fs::read(fonts.join(name)).ok());
    let Some(bytes) = bytes else {
        return false;
    };

    let name = "rustmoku-cjk".to_owned();
    let mut definitions = FontDefinitions::default();
    definitions
        .font_data
        .insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        definitions
            .families
            .get_mut(&family)
            .expect("default font family")
            .push(name.clone());
    }
    ctx.set_fonts(definitions);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustmoku_engine::{Proof, ProofSource, SearchStatistics};

    fn search_info(origin: SearchOrigin, proof: Option<Proof>) -> SearchInfo {
        SearchInfo {
            completed_depth: 0,
            seldepth: 0,
            best_move: None,
            score: 0,
            principal_variation: Vec::new(),
            statistics: SearchStatistics::default(),
            proof,
            origin,
        }
    }

    #[test]
    fn locale_mapping_and_representative_translations_are_centralized() {
        for locale in ["zh", "zh-CN", "zh_SG.UTF-8", "zh-Hans-CN"] {
            assert_eq!(
                LanguagePreference::Auto.resolve(Some(locale)),
                UiLanguage::SimplifiedChinese
            );
        }
        for locale in [None, Some("en-US"), Some("zh-TW"), Some("ja-JP")] {
            assert_eq!(
                LanguagePreference::Auto.resolve(locale),
                UiLanguage::English
            );
        }
        assert_eq!(
            LanguagePreference::English.resolve(Some("zh-CN")),
            UiLanguage::English
        );
        assert_eq!(
            UiText::new(UiLanguage::English).get(TextKey::NewGame),
            "New Game"
        );
        assert_eq!(
            UiText::new(UiLanguage::SimplifiedChinese).get(TextKey::NewGame),
            "新对局"
        );
        assert_eq!(
            UiText::new(UiLanguage::SimplifiedChinese).get(TextKey::Waiting),
            "AI：等待完成搜索深度"
        );
    }

    #[test]
    fn result_origins_and_proof_distances_are_worded_honestly() {
        let english = UiText::new(UiLanguage::English);
        let chinese = UiText::new(UiLanguage::SimplifiedChinese);
        assert_eq!(
            english.result_summary(&search_info(SearchOrigin::AlphaBeta, None)),
            "Result: Alpha-Beta"
        );
        assert_eq!(
            chinese.result_summary(&search_info(SearchOrigin::Immediate, None)),
            "结果来源：即时精确战术"
        );
        assert_eq!(
            english.result_summary(&search_info(
                SearchOrigin::Vcf,
                Some(Proof {
                    source: ProofSource::Vcf,
                    distance: ProofDistance::Exact(5),
                }),
            )),
            "Proof: VCF · exact 5 plies"
        );
        assert_eq!(
            chinese.result_summary(&search_info(
                SearchOrigin::ProofBook,
                Some(Proof {
                    source: ProofSource::ProofBook,
                    distance: ProofDistance::AtMost(17),
                }),
            )),
            "证明：已验证证明库 · 最多 17 层"
        );
    }
}
