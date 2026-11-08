use crate::{api, model::*, native, offline, storage};
use GridLength::{Auto, Star};
use HorizontalAlignment::Stretch;
use VerticalAlignment::Center;
use windows_reactor::*;

#[derive(Clone, Default)]
pub struct WindowInput {
    pub settings_window: bool,
    pub tab: String,
    pub parent: Option<LocalSender<Message>>,
    pub initial_text: String,
}
impl PartialEq for WindowInput {
    fn eq(&self, other: &Self) -> bool {
        self.settings_window == other.settings_window
            && self.tab == other.tab
            && self.initial_text == other.initial_text
    }
}
#[derive(Clone)]
pub enum Message {
    Text(String),
    Translate,
    Debounced(u64),
    Finished(
        u64,
        Result<Translation, String>,
        Option<crate::dictionary::Entry>,
    ),
    Details(bool),
    Pronounce(crate::speech::Accent),
    Pronounced(u64, Result<(), String>),
    PhoneticsReady(u64, Result<crate::speech::Phonetics, String>),
    ColorScheme(ColorScheme),
    RepairCaret(u8),
    RetryCaret(u8),
    Dismiss,
    Drag,
    Show,
    CaptureSelection,
    SelectionCaptured(u64, Option<String>),
    Quit,
    Composition(bool),
    OpenSettings,
    SettingsClosed,
    Reload,
    Tab(Option<String>),
    Swap,
    Provider(Option<usize>),
    ProviderPicker(bool),
    CloseProviderPicker(u64),
    Source(Option<usize>),
    Target(Option<usize>),
    AutoPair(bool),
    AutoStart(bool),
    SetId(String),
    SetKey(String),
    Region(String),
    AutoOffline(bool),
    DeepLFree(bool),
    Save,
    Theme(Option<usize>),
    Search(String),
    Restore(usize),
    Star(usize),
    Warmed,
}
pub struct Translator {
    settings_window: bool,
    parent: Option<LocalSender<Message>>,
    settings_open: bool,
    tab: String,
    text: String,
    force_online_for_long_text: bool,
    pending_provider: Option<Provider>,
    output: Option<Translation>,
    entry: Option<crate::dictionary::Entry>,
    details_open: bool,
    provider_picker_open: bool,
    provider_picker_selected: Option<Provider>,
    provider_picker_revision: u64,
    color_scheme: ColorScheme,
    settings: Settings,
    history: Vec<Translation>,
    query: String,
    busy: bool,
    revision: u64,
    task: Option<ComponentTask>,
    speech_task: Option<ComponentTask>,
    speech_revision: u64,
    speech_error: Option<String>,
    phonetics: Option<crate::speech::Phonetics>,
    phonetics_task: Option<ComponentTask>,
    selection_task: Option<ComponentTask>,
    selection_revision: u64,
    status: String,
    error: bool,
    input: ElementRef<TextBox>,
}
fn text(value: impl Into<String>, size: f64) -> TextBlock {
    TextBlock::new()
        .text(value)
        .font_size(size)
        .text_wrapping(TextWrapping::Wrap)
}
fn muted(value: impl Into<String>) -> TextBlock {
    text(value, 12.0).opacity(0.6)
}
fn popup_language(language: &Language) -> &'static str {
    if language.code == "zh" {
        "中文"
    } else {
        language.label
    }
}
fn meaning_key(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn meaning_parts(value: &str) -> impl Iterator<Item = &str> {
    value
        .split([',', '，', ';', '；', '、', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
}
fn chinese_senses(entry: &crate::dictionary::Entry) -> Vec<crate::dictionary::Sense> {
    let mut seen = std::collections::HashSet::new();
    let mut senses = Vec::new();
    for sense in &entry.senses {
        let definition = meaning_parts(&sense.definition)
            .filter(|part| part.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c)))
            .filter(|part| seen.insert((sense.part_of_speech.clone(), meaning_key(part))))
            .collect::<Vec<_>>()
            .join("，");
        if !definition.is_empty() {
            senses.push(crate::dictionary::Sense {
                part_of_speech: sense.part_of_speech.clone(),
                definition,
            });
        }
    }
    senses
}
fn visual_lines(value: &str, font_size: f64, width: f64) -> usize {
    value
        .split('\n')
        .map(|line| {
            let mut lines = 1;
            let mut used = 0.0;
            for c in line.chars().filter(|c| *c != '\r') {
                let advance = if c == '\t' {
                    font_size * 2.0
                } else if c.is_whitespace() {
                    font_size * 0.35
                } else if c >= '\u{1100}' {
                    font_size
                } else {
                    font_size * 0.6
                };
                if used + advance > width && used > 0.0 {
                    lines += 1;
                    used = 0.0;
                }
                used += advance;
            }
            lines
        })
        .sum::<usize>()
        .max(1)
}
fn dictionary_metadata(entry: &crate::dictionary::Entry) -> String {
    entry.parts_of_speech.join(" · ")
}
fn at(view: impl Into<View>, row: i32, column: i32) -> View {
    Grid::new()
        .grid_row(row)
        .grid_column(column)
        .children((view.into(),))
}
// Native Fluent vector glyphs use their intrinsic size and centered layout.
const TRANSLATE: &str = "\u{E8C1}";
const SWAP: &str = "\u{E8AB}";
const SETTINGS: &str = "\u{E713}";
const HISTORY: &str = "\u{E81C}";
fn icon(path: &str) -> FontIcon {
    FontIcon::new().glyph(path)
}
fn provider_icon(provider: Provider) -> Image {
    let data: &'static [u8] = match provider {
        Provider::Tencent => include_bytes!("../assets/providers/tencent.png"),
        Provider::Baidu => include_bytes!("../assets/providers/baidu.png"),
        Provider::Youdao => include_bytes!("../assets/providers/youdao.png"),
        Provider::DeepL => include_bytes!("../assets/providers/deepl.png"),
        Provider::Offline => include_bytes!("../assets/providers/local.png"),
        Provider::MyMemory => include_bytes!("../assets/providers/mymemory.png"),
    };
    Image::new()
        .source_data(EncodedImage::from_static(data))
        .width(18.0)
        .height(18.0)
        .stretch(windows_reactor::Stretch::Uniform)
}
fn provider_configured(settings: &Settings, provider: Provider) -> bool {
    !provider.needs_key()
        || settings
            .credentials
            .get(provider.index())
            .is_some_and(|credential| {
                !credential.key.trim().is_empty()
                    && (provider == Provider::DeepL || !credential.id.trim().is_empty())
            })
}
fn field(label: &str, control: impl Into<View>) -> View {
    StackPanel::new()
        .spacing(5.0)
        .children((text(label, 13.0), control.into()))
}
fn icon_button(path: &str, name: &str, message: Message, cx: &ViewContext<Translator>) -> View {
    Button::new()
        .style(ButtonStyle::Subtle)
        .width(44.0)
        .height(44.0)
        .padding(6.0)
        .horizontal_content_alignment(HorizontalAlignment::Center)
        .vertical_content_alignment(Center)
        .automation_name(name)
        .on_click(cx.message(message))
        .content(icon(path))
        .tooltip(name)
}
fn theme(settings: &Settings) -> WindowTheme {
    match settings.theme {
        1 => WindowTheme::Light,
        2 => WindowTheme::Dark,
        _ => WindowTheme::System,
    }
}
impl Translator {
    fn cancel_selection(&mut self) {
        self.selection_revision = self.selection_revision.wrapping_add(1);
        if let Some(task) = self.selection_task.take() {
            task.cancel();
        }
    }

    fn cancel_speech(&mut self) {
        self.speech_revision = self.speech_revision.wrapping_add(1);
        if let Some(task) = self.speech_task.take() {
            task.cancel();
        }
        self.speech_error = None;
        crate::speech::stop();
    }

    fn pronunciation_text(&self) -> Option<&str> {
        let translation = self.output.as_ref()?;
        if self.busy || self.text.trim() != translation.source.trim() {
            return None;
        }
        if translation.from == "en" {
            Some(translation.source.trim())
        } else if translation.to == "en" {
            Some(translation.target.trim())
        } else {
            None
        }
    }

    fn load_phonetics(&mut self, cx: &ComponentContext<Self>) {
        if let Some(task) = self.phonetics_task.take() {
            task.cancel();
        }
        self.phonetics = None;
        if let Some(text) = self.pronunciation_text().map(str::to_owned) {
            let revision = self.revision;
            self.phonetics_task = Some(cx.spawn_background(move |cancel| {
                Message::PhoneticsReady(
                    revision,
                    crate::speech::phonetics(&text, &|| cancel.is_cancelled()),
                )
            }));
        }
    }

    fn pronunciation_controls(&self, cx: &ViewContext<Self>) -> View {
        if self.pronunciation_text().is_none() {
            return View::empty();
        }
        Grid::new()
            .columns([Auto, Auto])
            .column_spacing(8.0)
            .horizontal_alignment(HorizontalAlignment::Left)
            .keyed_children(
                [
                    (crate::speech::Accent::British, "英", "英式发音"),
                    (crate::speech::Accent::American, "美", "美式发音"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (accent, label, name))| {
                    let phonetic = self.phonetics.as_ref().map_or("", |value| match accent {
                        crate::speech::Accent::British => value.british.as_str(),
                        crate::speech::Accent::American => value.american.as_str(),
                    });
                    let phonetic = phonetic.trim().trim_matches('/');
                    let phonetic = if phonetic.is_empty() {
                        String::new()
                    } else {
                        format!("/{phonetic}/")
                    };
                    (
                        index,
                        at(
                            Button::new()
                                .style(ButtonStyle::Subtle)
                                .height(32.0)
                                .min_width(0.0)
                                .max_width(160.0)
                                .padding(4.0)
                                .horizontal_alignment(HorizontalAlignment::Left)
                                .horizontal_content_alignment(HorizontalAlignment::Left)
                                .vertical_content_alignment(Center)
                                .automation_name(name)
                                .on_click(cx.message(Message::Pronounce(accent)))
                                .content(
                                    Grid::new()
                                        .columns([Auto, Auto, Auto])
                                        .column_spacing(6.0)
                                        .children((
                                            text(label, 14.0)
                                                .opacity(0.65)
                                                .vertical_alignment(Center),
                                            text(&phonetic, 14.0)
                                                .max_width(96.0)
                                                .text_wrapping(TextWrapping::NoWrap)
                                                .text_trimming(TextTrimming::CharacterEllipsis)
                                                .opacity(0.65)
                                                .vertical_alignment(Center)
                                                .grid_column(1),
                                            icon("\u{E767}")
                                                .vertical_alignment(Center)
                                                .grid_column(2),
                                        )),
                                )
                                .tooltip(format!("{name} {phonetic}")),
                            0,
                            index as i32,
                        ),
                    )
                }),
            )
    }

    fn invalidate(&mut self) {
        self.revision += 1;
        self.busy = false;
        self.phonetics = None;
        if let Some(task) = self.phonetics_task.take() {
            task.cancel();
        }
        if let Some(task) = self.task.take() {
            task.cancel();
        }
    }
    fn save(&mut self) {
        match storage::save_settings(&self.settings) {
            Ok(()) => {
                self.status = "已保存".into();
                self.error = false;
                if let Some(parent) = &self.parent {
                    let _ = parent.send(Message::Reload);
                }
            }
            Err(e) => {
                self.status = format!("保存失败：{e}");
                self.error = true;
            }
        }
    }
    fn schedule(&mut self, cx: &ComponentContext<Self>) {
        self.invalidate();
        if self.settings_window {
            return;
        }
        let revision = self.revision;
        self.task = Some(cx.spawn_background(move |cancel| {
            for _ in 0..7 {
                if cancel.is_cancelled() {
                    return Message::Warmed;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Message::Debounced(revision)
        }));
    }
    fn language_indices(&self) -> (usize, usize) {
        if self.settings.auto_pair {
            if self.text.trim().is_empty() || detect(&self.text) == "zh" {
                (1, 2)
            } else {
                (2, 1)
            }
        } else {
            (
                self.settings.from_index.min(LANGUAGES.len() - 1),
                self.settings.to_index.clamp(1, LANGUAGES.len() - 1),
            )
        }
    }
    fn translate(&mut self, cx: &ComponentContext<Self>) {
        if self.busy || self.text.trim().is_empty() {
            return;
        }
        self.invalidate();
        self.busy = true;
        self.pending_provider = Some(
            if self.text.chars().count() > api::ONLINE_TEXT_LIMIT
                && !self.force_online_for_long_text
            {
                Provider::Offline
            } else {
                self.settings.provider
            },
        );
        let revision = self.revision;
        let text = self.text.clone();
        let settings = self.settings.clone();
        let force_online_for_long_text = self.force_online_for_long_text;
        let (source, target) = self.language_indices();
        let from = LANGUAGES[source].code.to_owned();
        let to = LANGUAGES[target].code.to_owned();
        self.task = Some(cx.spawn_background(move |cancel| {
            let entry = if from == "en" || from == "auto" {
                crate::dictionary::lookup(&text)
            } else {
                None
            };
            let result = if force_online_for_long_text {
                api::translate_with_policy(text, from, to, settings, true, &|| {
                    cancel.is_cancelled()
                })
            } else {
                api::translate_cancellable(text, from, to, settings, &|| cancel.is_cancelled())
            };
            Message::Finished(revision, result, entry)
        }));
    }
    fn card(&self, content: impl Into<View>) -> View {
        Border::new()
            .background(ThemeBrush::CardBackground)
            .border_brush(ThemeBrush::CardStroke)
            .border_thickness(1.0)
            .corner_radius(8.0)
            .padding(14.0)
            .content(content)
    }
    fn provider_choices(&self, selected: Provider, compact: bool, cx: &ViewContext<Self>) -> View {
        NavigationView::new()
            .width(if compact { 320.0 } else { 420.0 })
            .height(if compact { 264.0 } else { 144.0 })
            .horizontal_alignment(HorizontalAlignment::Left)
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .open_pane_length(if compact { 320.0 } else { 420.0 })
            .menu_columns(if compact { 1 } else { 3 })
            .is_pane_open(true)
            .is_pane_toggle_button_visible(false)
            .is_back_button_visible(NavigationViewBackButtonVisible::Collapsed)
            .is_settings_visible(false)
            .always_show_header(false)
            .on_selected_tag_changed(cx.callback(|tag: Option<String>| {
                Message::Provider(tag.and_then(|value| value.parse::<usize>().ok()))
            }))
            .slots([SlotView::collection(
                NavigationViewSlot::MenuItems,
                Provider::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, provider)| {
                        let configured = provider_configured(&self.settings, provider);
                        let selectable = !compact || configured;
                        let label = if compact && !configured {
                            format!("{} · 未配置", provider.name())
                        } else {
                            provider.name().to_owned()
                        };
                        let status = match provider {
                            Provider::Offline if offline::ready() => "已就绪",
                            Provider::Offline => "未就绪",
                            Provider::MyMemory => "免密钥试用",
                            _ if configured => "已配置",
                            _ => "未配置",
                        };
                        let content = if compact {
                            StackPanel::new()
                                .orientation(Orientation::Horizontal)
                                .spacing(10.0)
                                .children((
                                    provider_icon(provider).vertical_alignment(Center),
                                    text(label.clone(), 14.0)
                                        .text_wrapping(TextWrapping::NoWrap)
                                        .vertical_alignment(Center),
                                ))
                        } else {
                            StackPanel::new()
                                .orientation(Orientation::Horizontal)
                                .spacing(8.0)
                                .children((
                                    provider_icon(provider).vertical_alignment(Center),
                                    StackPanel::new()
                                        .spacing(2.0)
                                        .vertical_alignment(Center)
                                        .children((
                                            text(provider.name(), 13.0)
                                                .text_wrapping(TextWrapping::NoWrap),
                                            text(status, 11.0)
                                                .opacity(0.6)
                                                .text_wrapping(TextWrapping::NoWrap),
                                        )),
                                ))
                        };
                        (
                            index,
                            NavigationViewItem::new()
                                .tag(index.to_string())
                                .is_selected(provider == selected)
                                .selects_on_invoked(selectable)
                                .opacity(if selectable { 1.0 } else { 0.45 })
                                .automation_name(if compact {
                                    label
                                } else {
                                    format!("{}，{status}", provider.name())
                                })
                                .slot(NavigationViewItemSlot::Content, content),
                        )
                    }),
            )])
    }
    fn close_provider_picker(&mut self) {
        self.provider_picker_open = false;
        self.provider_picker_selected = None;
        self.provider_picker_revision = self.provider_picker_revision.wrapping_add(1);
        if !self.settings_window {
            let _ = self.input.request_focus();
        }
    }
    fn open_settings(&mut self, cx: &ComponentContext<Self>) {
        if self.settings_open && native::activate_settings() {
            return;
        }
        self.settings_open = cx.open_window(View::component::<Self>(WindowInput {
            settings_window: true,
            tab: "general".into(),
            parent: Some(cx.sender()),
            initial_text: String::new(),
        }));
    }
    fn repair_caret(&self, cx: &ComponentContext<Self>, attempt: u8) {
        let dark = self.settings.theme == 2
            || (self.settings.theme == 0 && self.color_scheme == ColorScheme::Dark);
        let color = if dark {
            Color::rgb(245, 245, 245)
        } else {
            Color::rgb(24, 24, 24)
        };
        let sender = cx.sender();
        let _ = self.input.request_caret_color_result(color, move |result| {
            if matches!(result, Ok(false)) && attempt < 5 {
                let _ = sender.send(Message::RetryCaret(attempt + 1));
            }
        });
    }
    fn popup(&self, cx: &mut ViewContext<Self>) -> View {
        cx.on_color_scheme(cx.callback(Message::ColorScheme));
        let dark = self.settings.theme == 2
            || (self.settings.theme == 0 && self.color_scheme == ColorScheme::Dark);
        let placeholder = if dark {
            Color::argb(100, 255, 255, 255)
        } else {
            Color::argb(90, 0, 0, 0)
        };
        let expanded = self.output.is_some();
        let dictionary_senses = self
            .output
            .as_ref()
            .and(self.entry.as_ref())
            .map(chinese_senses)
            .unwrap_or_default();
        let details_open = self.details_open && !dictionary_senses.is_empty();
        let is_word = self.output.as_ref().is_some_and(|translation| {
            !translation.source.trim().is_empty()
                && translation
                    .source
                    .trim()
                    .chars()
                    .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '\'')
        });
        let long_input = self.text.chars().count() > 80
            || visual_lines(&self.text, 23.0, 502.0) > 2
            || self
                .text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
                > 1;
        let input_font = if long_input { 15.0 } else { 23.0 };
        let input_line_height = if long_input { 21.0 } else { 32.0 };
        let input_height = (visual_lines(&self.text, input_font, 502.0) as f64 * input_line_height
            + 12.0)
            .clamp(60.0, 180.0);
        let fixed_height = 119.0 + input_height;
        let available_result = (480.0 - fixed_height).max(80.0);
        let result_font = self.output.as_ref().map_or(23.0, |translation| {
            if long_input
                || translation.target.chars().count() > 80
                || visual_lines(&translation.target, 23.0, 472.0) > 2
                || translation
                    .target
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count()
                    > 1
            {
                15.0
            } else {
                23.0
            }
        });
        let result_line_height = if result_font == 15.0 { 21.0 } else { 32.0 };
        let inline_pronunciation = self.pronunciation_text().is_some()
            && self.entry.is_some()
            && self.output.as_ref().is_some_and(|translation| {
                visual_lines(&translation.target, result_font, 120.0) == 1
            });
        let pronunciation_height = if self.pronunciation_text().is_some() && !inline_pronunciation {
            40.0
        } else {
            0.0
        };
        let result_notice = self.output.as_ref().and_then(|translation| {
            if translation.note.contains("后续内容未翻译") {
                Some("在线仅翻译前5000字符，后续内容未翻译")
            } else if translation.note.contains("在线不可用，已使用本地翻译全文") {
                Some("在线不可用，已使用本地翻译全文")
            } else if translation.note.contains("长文本已自动使用本地翻译") {
                Some("长文本已自动使用本地翻译")
            } else {
                None
            }
        });
        let notice_height = result_notice.map_or(0.0, |notice| {
            visual_lines(notice, 12.0, 496.0) as f64 * 18.0 + 6.0
        });
        let metadata = self
            .entry
            .as_ref()
            .map(dictionary_metadata)
            .filter(|value| !value.is_empty());
        let mut target_max_height = available_result;
        let mut details_viewport_height = 0.0;
        let result_height = if let Some(translation) = &self.output {
            if self.entry.is_some() {
                let metadata_height = metadata.as_ref().map_or(0.0, |value| {
                    let width = if inline_pronunciation { 120.0 } else { 472.0 };
                    visual_lines(value, 14.0, width) as f64 * 20.0 + 6.0
                });
                let header_fixed = 28.0 + metadata_height + pronunciation_height;
                let detail_reserve = if details_open { 72.0 } else { 0.0 };
                let target_limit =
                    (available_result - header_fixed - detail_reserve).max(result_line_height);
                target_max_height = (visual_lines(&translation.target, result_font, 472.0) as f64
                    * result_line_height)
                    .min(target_limit);
                let header_height = header_fixed + target_max_height;
                if details_open {
                    let meanings_height = dictionary_senses
                        .iter()
                        .map(|sense| visual_lines(&sense.definition, 15.0, 440.0) as f64 * 21.0)
                        .sum::<f64>()
                        + dictionary_senses.len().saturating_sub(1) as f64 * 12.0;
                    let details_height =
                        (meanings_height + 25.0).min((available_result - header_height).max(25.0));
                    details_viewport_height = (details_height - 25.0).max(0.0);
                    header_height + details_height
                } else {
                    header_height
                }
            } else {
                let word_metadata = if is_word { 26.0 } else { 0.0 };
                (30.0
                    + notice_height
                    + pronunciation_height
                    + word_metadata
                    + visual_lines(&translation.target, result_font, 496.0) as f64
                        * result_line_height)
                    .min(available_result)
            }
        } else if self.error {
            (visual_lines(&self.status, 12.0, 502.0).min(4) as f64 * 18.0 + 4.0)
                .min(available_result)
        } else {
            0.0
        };
        let natural_height = if expanded || self.error {
            (fixed_height + result_height).clamp(184.0, 480.0)
        } else {
            fixed_height.max(184.0).min(480.0)
        };
        let height = if self.provider_picker_open {
            natural_height.max(322.0)
        } else {
            natural_height
        };
        cx.window_title("翻译");
        cx.window_visuals(
            WindowVisuals::new()
                .client_size(558.0, 184.0)
                .backdrop(WindowBackdrop::Mica)
                .theme(theme(&self.settings)),
        );
        let dismiss = cx.message(Message::Dismiss);
        cx.use_effect(
            "popup-frame",
            (
                expanded,
                self.error,
                details_open,
                self.provider_picker_open,
                height.round() as u32,
            ),
            move || {
                native::configure_popup(558.0, height);
                native::escape_handler(move || {
                    let _ = dismiss.call(());
                });
                None
            },
        );
        let input_focus = self.input.clone();
        cx.use_effect("input-focus", (), move || {
            let _ = input_focus.request_focus();
            None
        });
        let caret_sender = cx.sender();
        cx.use_effect("input-caret", (dark, self.text.clone()), move || {
            let _ = caret_sender.send(Message::RepairCaret(0));
            None
        });
        let (from, to) = self.language_indices();
        let languages = Grid::new()
            .columns([Auto, Auto, Auto, Auto, Star(1.0), Auto])
            .column_spacing(4.0)
            .children((
                Border::new()
                    .width(44.0)
                    .height(44.0)
                    .vertical_alignment(Center)
                    .content(
                        icon(TRANSLATE)
                            .horizontal_alignment(HorizontalAlignment::Center)
                            .vertical_alignment(Center),
                    ),
                ComboBox::new()
                    .items_source(LANGUAGES.iter().map(popup_language))
                    .selected_index(from)
                    .width(144.0)
                    .height(44.0)
                    .vertical_alignment(Center)
                    .automation_name("源语言")
                    .on_selection_changed(cx.callback(Message::Source))
                    .grid_column(1),
                at(icon_button(SWAP, "交换语言", Message::Swap, cx), 0, 2),
                ComboBox::new()
                    .items_source(LANGUAGES.iter().skip(1).map(popup_language))
                    .selected_index(to - 1)
                    .width(132.0)
                    .height(44.0)
                    .vertical_alignment(Center)
                    .automation_name("目标语言")
                    .on_selection_changed(cx.callback(Message::Target))
                    .grid_column(3),
                at(
                    icon_button(SETTINGS, "设置", Message::OpenSettings, cx),
                    0,
                    5,
                ),
            ));
        let input = TextBox::new()
            .element_ref(&self.input)
            .font_size(input_font)
            .resource_overrides(
                [
                    "TextControlBackground",
                    "TextControlBackgroundPointerOver",
                    "TextControlBackgroundFocused",
                    "TextControlBackgroundDisabled",
                    "TextControlBorderBrush",
                    "TextControlBorderBrushPointerOver",
                    "TextControlBorderBrushFocused",
                    "TextControlBorderBrushDisabled",
                ]
                .into_iter()
                .fold(ResourceOverrides::new(), |resources, key| {
                    resources.set(key, ResourceValue::Brush(Color::transparent()))
                })
                .set(
                    "TextControlPlaceholderForeground",
                    ResourceValue::Brush(placeholder),
                )
                .set(
                    "TextControlPlaceholderForegroundPointerOver",
                    ResourceValue::Brush(placeholder),
                )
                .set(
                    "TextControlPlaceholderForegroundFocused",
                    ResourceValue::Brush(placeholder),
                ),
            )
            .text(&self.text)
            .placeholder_text("输入单词或文本")
            .placeholder_foreground(placeholder)
            .accepts_return(true)
            .text_wrapping(TextWrapping::Wrap)
            .background(Color::transparent())
            .border_thickness(0.0)
            .border_brush(Color::transparent())
            .min_height(60.0)
            .max_height(180.0)
            .horizontal_alignment(Stretch)
            .automation_name("输入单词或文本")
            .on_text_changed(cx.callback(Message::Text));
        let result: View = if let Some(translation) = &self.output {
            if let Some(entry) = &self.entry {
                let summary = StackPanel::new().spacing(6.0).children((
                    if let Some(value) = &metadata {
                        text(value, 14.0).opacity(0.64).tooltip(&entry.word)
                    } else {
                        View::empty()
                    },
                    ScrollViewer::new()
                        .max_height(target_max_height)
                        .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                        .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                        .content(text(&translation.target, result_font)),
                ));
                let has_details = !dictionary_senses.is_empty();
                let details_label = if details_open {
                    "收起词典释义"
                } else {
                    "展开词典释义"
                };
                let chevron = if details_open { "\u{E70E}" } else { "\u{E70D}" };
                let header: View = if inline_pronunciation {
                    let summary = Border::new().max_width(120.0).content(summary);
                    let summary: View = if has_details {
                        Button::new()
                            .style(ButtonStyle::Subtle)
                            .min_width(0.0)
                            .horizontal_content_alignment(HorizontalAlignment::Left)
                            .padding(Thickness::new(12.0, 12.0, 4.0, 12.0))
                            .automation_name(details_label)
                            .on_click(cx.message(Message::Details(!details_open)))
                            .content(summary)
                            .into()
                    } else {
                        Border::new()
                            .padding(Thickness::new(12.0, 12.0, 4.0, 12.0))
                            .content(summary)
                    };
                    Grid::new()
                        .columns([Auto, Auto, Star(1.0), Auto])
                        .column_spacing(8.0)
                        .children((
                            summary,
                            Border::new()
                                .vertical_alignment(VerticalAlignment::Bottom)
                                .margin(Thickness::new(0.0, 0.0, 0.0, 12.0))
                                .grid_column(1)
                                .content(self.pronunciation_controls(cx)),
                            if has_details {
                                Button::new()
                                    .style(ButtonStyle::Subtle)
                                    .min_width(0.0)
                                    .width(32.0)
                                    .height(32.0)
                                    .padding(6.0)
                                    .margin(Thickness::new(0.0, 0.0, 8.0, 12.0))
                                    .vertical_alignment(VerticalAlignment::Bottom)
                                    .horizontal_content_alignment(HorizontalAlignment::Center)
                                    .vertical_content_alignment(Center)
                                    .automation_name(details_label)
                                    .on_click(cx.message(Message::Details(!details_open)))
                                    .grid_column(3)
                                    .content(icon(chevron).opacity(0.6))
                                    .into()
                            } else {
                                View::empty()
                            },
                        ))
                        .into()
                } else {
                    let header_content = Grid::new()
                        .columns([Star(1.0), Auto])
                        .column_spacing(8.0)
                        .children((
                            summary,
                            if has_details {
                                icon(chevron)
                                    .opacity(0.6)
                                    .vertical_alignment(Center)
                                    .grid_column(1)
                                    .into()
                            } else {
                                View::empty()
                            },
                        ));
                    if has_details {
                        Button::new()
                            .style(ButtonStyle::Subtle)
                            .horizontal_alignment(Stretch)
                            .horizontal_content_alignment(Stretch)
                            .padding(12.0)
                            .automation_name(details_label)
                            .on_click(cx.message(Message::Details(!details_open)))
                            .content(header_content)
                            .into()
                    } else {
                        Border::new().padding(12.0).content(header_content)
                    }
                };
                let details: View = if details_open {
                    let senses = StackPanel::new().spacing(12.0).keyed_children(
                        dictionary_senses.iter().enumerate().map(|(index, sense)| {
                            (
                                index,
                                Grid::new()
                                    .columns([GridLength::Pixel(42.0), Star(1.0)])
                                    .column_spacing(8.0)
                                    .children((
                                        text(&sense.part_of_speech, 14.0).opacity(0.6),
                                        text(&sense.definition, 15.0)
                                            .is_text_selection_enabled(true)
                                            .grid_column(1),
                                    )),
                            )
                        }),
                    );
                    Border::new()
                        .border_brush(ThemeBrush::CardStroke)
                        .border_thickness(Thickness::new(0.0, 1.0, 0.0, 0.0))
                        .padding(12.0)
                        .content(
                            ScrollViewer::new()
                                .max_height(details_viewport_height)
                                .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                                .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                                .content(
                                    Border::new()
                                        .padding(Thickness::new(0.0, 0.0, 10.0, 0.0))
                                        .content(senses),
                                ),
                        )
                } else {
                    View::empty()
                };
                Border::new()
                    .background(ThemeBrush::CardBackground)
                    .border_brush(ThemeBrush::CardStroke)
                    .border_thickness(1.0)
                    .corner_radius(8.0)
                    .max_height(result_height)
                    .vertical_alignment(VerticalAlignment::Top)
                    .content(Grid::new().rows([Auto, Auto, Auto]).children((
                        header,
                        if pronunciation_height > 0.0 {
                            Border::new()
                                .padding(Thickness::new(12.0, 0.0, 12.0, 8.0))
                                .grid_row(1)
                                .content(self.pronunciation_controls(cx))
                        } else {
                            View::empty()
                        },
                        at(details, 2, 0),
                    )))
            } else {
                Border::new()
                    .background(ThemeBrush::CardBackground)
                    .border_brush(ThemeBrush::CardStroke)
                    .border_thickness(1.0)
                    .corner_radius(8.0)
                    .padding(14.0)
                    .max_height(result_height)
                    .vertical_alignment(VerticalAlignment::Top)
                    .content(
                        StackPanel::new().spacing(6.0).children((
                            if let Some(notice) = result_notice {
                                text(notice, 12.0).opacity(0.75).into()
                            } else {
                                View::empty()
                            },
                            ScrollViewer::new()
                                .max_height(
                                    (result_height - 30.0 - notice_height - pronunciation_height)
                                        .max(32.0),
                                )
                                .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                                .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                                .content(
                                    StackPanel::new().spacing(6.0).children((
                                        if is_word {
                                            text("词性未收录", 14.0).opacity(0.64).into()
                                        } else {
                                            View::empty()
                                        },
                                        text(&translation.target, result_font)
                                            .is_text_selection_enabled(true),
                                    )),
                                ),
                            self.pronunciation_controls(cx),
                        )),
                    )
            }
        } else if self.error {
            ScrollViewer::new()
                .max_height(result_height)
                .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                .content(
                    text(&self.status, 12.0)
                        .opacity(0.8)
                        .margin(Thickness::new(6.0, 2.0, 6.0, 2.0)),
                )
        } else {
            View::empty()
        };
        let engine = self
            .output
            .as_ref()
            .map(|translation| {
                format!(
                    "{}{}",
                    translation.provider.name(),
                    if translation.note.contains("尚未配置") {
                        " · 未配置密钥"
                    } else if translation.note.contains("网络") {
                        " · 网络不可用"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_else(|| self.settings.provider.name().into());
        let actual_provider = self
            .output
            .as_ref()
            .map_or(self.settings.provider, |translation| translation.provider);
        let provider = self.pending_provider.unwrap_or(actual_provider);
        let state = Button::new()
            .style(ButtonStyle::Subtle)
            .height(32.0)
            .min_height(32.0)
            .padding(Thickness::new(6.0, 4.0, 6.0, 4.0))
            .horizontal_alignment(HorizontalAlignment::Left)
            .automation_name("选择翻译服务")
            .on_click(cx.message(Message::ProviderPicker(!self.provider_picker_open)))
            .content(
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(6.0)
                    .children((
                        provider_icon(provider),
                        muted(if let Some(pending) = self.pending_provider {
                            if self.busy {
                                format!("{} · 翻译中…", pending.name())
                            } else {
                                pending.name().to_owned()
                            }
                        } else if self.busy {
                            "翻译中…".into()
                        } else {
                            engine
                        })
                        .vertical_alignment(Center),
                    )),
            )
            .tooltip("选择翻译服务");
        let state = Grid::new().keyed_children([(self.settings.provider.index(), state)]);
        let footer = Grid::new()
            .columns([Auto, Star(1.0)])
            .column_spacing(8.0)
            .children((
                state,
                if let Some(error) = &self.speech_error {
                    muted("语音暂不可用")
                        .max_lines(1)
                        .vertical_alignment(Center)
                        .grid_column(1)
                        .tooltip(error)
                } else {
                    View::empty()
                },
            ));
        let surface = Border::new()
            .corner_radius(12.0)
            .padding(Thickness::new(14.0, 12.0, 14.0, 10.0))
            .on_pointer_pressed(cx.callback(|point: PointerEventInfo| {
                if point.window_y < 8.0 {
                    Message::Drag
                } else {
                    Message::Warmed
                }
            }))
            .content(
                Grid::new()
                    .rows([
                        Auto,
                        Auto,
                        if self.provider_picker_open {
                            Star(1.0)
                        } else {
                            Auto
                        },
                        Auto,
                    ])
                    .row_spacing(7.0)
                    .children((
                        languages,
                        at(input, 1, 0),
                        at(result, 2, 0),
                        at(footer, 3, 0),
                    )),
            );
        let dismiss_layer: View = if self.provider_picker_open {
            Border::new()
                .background(Color::transparent())
                .on_pointer_pressed(
                    cx.callback(|_: PointerEventInfo| Message::ProviderPicker(false)),
                )
                .content(View::empty())
        } else {
            View::empty()
        };
        let choices: View = if self.provider_picker_open {
            Border::new()
                .background(ThemeBrush::SolidBackground)
                .border_brush(ThemeBrush::CardStroke)
                .border_thickness(1.0)
                .corner_radius(8.0)
                .width(322.0)
                .height(266.0)
                .horizontal_alignment(HorizontalAlignment::Left)
                .vertical_alignment(VerticalAlignment::Bottom)
                .margin(Thickness::new(16.0, 0.0, 0.0, 44.0))
                .content(self.provider_choices(
                    self.provider_picker_selected.unwrap_or(provider),
                    true,
                    cx,
                ))
        } else {
            View::empty()
        };
        Grid::new().children((surface, dismiss_layer, choices))
    }
    fn provider_settings(&self, cx: &ViewContext<Self>) -> View {
        let provider = self.settings.provider;
        let credential = self.settings.credential();
        let credentials: View = if provider.needs_key() {
            let id_label = match provider {
                Provider::Tencent => "SecretId",
                Provider::Baidu => "APP ID",
                Provider::Youdao => "应用 ID",
                _ => "账户标识",
            };
            StackPanel::new().spacing(10.0).children((
                if provider == Provider::DeepL {
                    View::empty()
                } else {
                    field(
                        id_label,
                        TextBox::new()
                            .text(credential.id)
                            .automation_name("API 标识")
                            .on_text_changed(cx.callback(Message::SetId)),
                    )
                },
                field(
                    if provider == Provider::Tencent {
                        "SecretKey"
                    } else {
                        "API 密钥"
                    },
                    PasswordBox::new()
                        .password(credential.key)
                        .automation_name("API 密钥")
                        .on_password_changed(cx.callback(Message::SetKey)),
                ),
            ))
        } else {
            muted(if provider == Provider::Offline {
                "中文 ⇄ 英语"
            } else {
                "无需密钥，单次最多 450 字节"
            })
            .into()
        };
        let extra: View = match provider {
            Provider::Tencent => field(
                "地域",
                TextBox::new()
                    .text(&self.settings.region)
                    .on_text_changed(cx.callback(Message::Region)),
            ),
            Provider::DeepL => ToggleSwitch::new()
                .is_on(self.settings.deepl_free)
                .on_toggled(cx.callback(Message::DeepLFree))
                .slot(ToggleSwitchSlot::Header, "使用 Free API"),
            _ => View::empty(),
        };
        let routes = crate::routing::status(&self.settings);
        let route_rows = StackPanel::new()
            .spacing(10.0)
            .keyed_children(routes.into_iter().map(|route| {
                let description = if !route.configured {
                    "未配置".to_owned()
                } else if !route.eligible {
                    "未启用 Free API".to_owned()
                } else if route.cooldown_seconds > 0 {
                    format!("暂不可用 · {}", route.message)
                } else if let Some(remaining) = route.remaining_characters {
                    let period = if route.provider == Provider::MyMemory {
                        "今日"
                    } else {
                        "本月"
                    };
                    format!(
                        "{period}剩余 {remaining} / {} 字符",
                        route.allowance_characters
                    )
                } else {
                    route.message
                };
                (
                    route.provider.index(),
                    Grid::new()
                        .columns([GridLength::Pixel(90.0), Star(1.0)])
                        .column_spacing(8.0)
                        .children((
                            text(route.provider.name(), 13.0),
                            muted(description).grid_column(1),
                        )),
                )
            }));
        let order = crate::routing::candidates(&self.settings)
            .iter()
            .map(|provider| provider.name())
            .collect::<Vec<_>>()
            .join(" → ");
        let order = if order.is_empty() {
            "当前为手动离线".to_owned()
        } else if self.settings.auto_offline {
            format!("{order} → 本地离线")
        } else {
            order
        };
        StackPanel::new().spacing(10.0).children((
            self.card(
                StackPanel::new().spacing(12.0).children((
                    field("默认引擎", self.provider_choices(provider, false, cx)),
                    credentials,
                    extra,
                    Button::new()
                        .style(ButtonStyle::Accent)
                        .on_click(cx.message(Message::Save))
                        .content("保存凭据"),
                )),
            ),
            self.card(StackPanel::new().spacing(10.0).children((
                text("在线路由", 14.0),
                muted(order),
                route_rows,
                muted("只记录本程序的用量；免费额度不足时自动换服务。"),
            ))),
        ))
    }
    fn general_settings(&self, cx: &ViewContext<Self>) -> View {
        StackPanel::new().spacing(10.0).children((
            self.card(
                Grid::new()
                    .columns([Star(1.0), Auto])
                    .column_spacing(14.0)
                    .children((
                        StackPanel::new().spacing(4.0).children((
                            text("开机启动", 14.0),
                            muted("登录当前 Windows 用户时启动"),
                        )),
                        ToggleSwitch::new()
                            .is_on(self.settings.auto_start)
                            .automation_name("开机启动")
                            .on_toggled(cx.callback(Message::AutoStart))
                            .grid_column(1),
                    )),
            ),
            self.card(
                Grid::new()
                    .columns([Star(1.0), Auto])
                    .column_spacing(14.0)
                    .children((
                        StackPanel::new().spacing(4.0).children((
                            text("自动选择中英方向", 14.0),
                            muted("根据输入文字切换中文与英语"),
                        )),
                        ToggleSwitch::new()
                            .is_on(self.settings.auto_pair)
                            .automation_name("自动选择中英方向")
                            .on_toggled(cx.callback(Message::AutoPair))
                            .grid_column(1),
                    )),
            ),
            self.card(
                Grid::new()
                    .columns([Star(1.0), Auto])
                    .column_spacing(14.0)
                    .children((
                        StackPanel::new().spacing(4.0).children((
                            text("自动使用离线翻译", 14.0),
                            muted(if offline::ready() {
                                "断网或未配置密钥时使用 · 中英模型已就绪"
                            } else {
                                "模型文件缺失，请检查 models 文件夹"
                            }),
                        )),
                        ToggleSwitch::new()
                            .is_on(self.settings.auto_offline)
                            .automation_name("自动使用离线翻译")
                            .on_toggled(cx.callback(Message::AutoOffline))
                            .grid_column(1),
                    )),
            ),
            self.card(
                Grid::new()
                    .columns([Star(1.0), Auto])
                    .column_spacing(14.0)
                    .children((
                        text("主题", 14.0).vertical_alignment(Center),
                        ComboBox::new()
                            .items_source(["跟随系统", "浅色", "深色"])
                            .selected_index(self.settings.theme.min(2))
                            .width(142.0)
                            .automation_name("主题")
                            .on_selection_changed(cx.callback(Message::Theme))
                            .grid_column(1),
                    )),
            ),
        ))
    }
    fn history_view(&self, cx: &ViewContext<Self>) -> View {
        let query = self.query.to_lowercase();
        let entries: Vec<(usize, View)> = self
            .history
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.source.to_lowercase().contains(&query) || t.target.to_lowercase().contains(&query)
            })
            .map(|(i, t)| {
                (
                    i,
                    self.card(
                        StackPanel::new().spacing(8.0).children((
                            muted(format!("{} · {}", t.created, t.provider.name())),
                            text(&t.source, 14.0).max_lines(2),
                            text(&t.target, 18.0).is_text_selection_enabled(true),
                            StackPanel::new()
                                .orientation(Orientation::Horizontal)
                                .spacing(8.0)
                                .children((
                                    Button::new()
                                        .on_click(cx.message(Message::Restore(i)))
                                        .content("打开"),
                                    Button::new()
                                        .automation_name("收藏记录")
                                        .on_click(cx.message(Message::Star(i)))
                                        .content(if t.starred { "取消收藏" } else { "收藏" }),
                                )),
                        )),
                    ),
                )
            })
            .collect();
        Grid::new()
            .rows([Auto, Star(1.0)])
            .row_spacing(10.0)
            .children((
                TextBox::new()
                    .text(&self.query)
                    .placeholder_text("搜索记录")
                    .on_text_changed(cx.callback(Message::Search)),
                at(
                    ScrollViewer::new()
                        .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                        .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                        .content(
                            Border::new()
                                .padding(Thickness::new(0.0, 0.0, 12.0, 0.0))
                                .content(StackPanel::new().spacing(8.0).keyed_children(entries)),
                        ),
                    1,
                    0,
                ),
            ))
    }
    fn settings_view(&self, cx: &mut ViewContext<Self>) -> View {
        cx.window_title("翻译设置");
        cx.window_visuals(
            WindowVisuals::new()
                .client_size(760.0, 540.0)
                .backdrop(WindowBackdrop::Mica)
                .theme(theme(&self.settings))
                .constraints(WindowConstraints {
                    min_width: Some(680.0),
                    min_height: Some(480.0),
                    ..Default::default()
                }),
        );
        let items = [
            ("general", "常规", SETTINGS),
            ("providers", "翻译服务", TRANSLATE),
            ("history", "历史记录", HISTORY),
        ];
        let title = items
            .iter()
            .find(|i| i.0 == self.tab)
            .map_or("常规", |i| i.1);
        let content = match self.tab.as_str() {
            "providers" => self.provider_settings(cx),
            "history" => self.history_view(cx),
            _ => self.general_settings(cx),
        };
        let content = if self.tab == "history" {
            content
        } else {
            ScrollViewer::new()
                .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                .content(content)
        };
        NavigationView::new()
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .open_pane_length(174.0)
            .is_pane_open(true)
            .is_pane_toggle_button_visible(false)
            .is_back_button_visible(NavigationViewBackButtonVisible::Collapsed)
            .is_settings_visible(false)
            .always_show_header(false)
            .on_selected_tag_changed(cx.callback(Message::Tab))
            .slots([
                SlotView::collection(
                    NavigationViewSlot::MenuItems,
                    items.iter().map(|(tag, label, path)| {
                        (
                            *tag,
                            NavigationViewItem::new()
                                .tag(*tag)
                                .is_selected(self.tab == *tag)
                                .slots([
                                    SlotView::new(NavigationViewItemSlot::Content, *label),
                                    SlotView::new(NavigationViewItemSlot::Icon, icon(path)),
                                ]),
                        )
                    }),
                ),
                SlotView::new(
                    NavigationViewSlot::Content,
                    Grid::new()
                        .rows([Auto, Star(1.0), Auto])
                        .row_spacing(12.0)
                        .margin(16.0)
                        .children((
                            text(title, 23.0).font_weight(FontWeight::SEMI_BOLD),
                            at(content, 1, 0),
                            muted(&self.status).grid_row(2),
                        )),
                ),
            ])
    }
}
impl Component for Translator {
    type Input = WindowInput;
    type Message = Message;
    fn create(input: &WindowInput, cx: &ComponentContext<Self>) -> Self {
        let (mut settings, history, mut warning) = storage::load();
        if !input.settings_window && !input.initial_text.trim().is_empty() {
            settings.follow_bilingual_input(&input.initial_text);
            cx.spawn_background(|_| Message::Translate);
        }
        if !input.settings_window {
            let sender = cx.sender();
            native::install_handler(move |action| {
                let _ = sender.send(match action {
                    native::Action::Show => Message::Show,
                    native::Action::TranslateSelection => Message::CaptureSelection,
                    native::Action::Settings => Message::OpenSettings,
                    native::Action::Quit => Message::Quit,
                    native::Action::CompositionStarted => Message::Composition(true),
                    native::Action::CompositionEnded => Message::Composition(false),
                });
            });
            if let Some(message) = native::take_startup_warning() {
                warning = Some(message);
            }
            if let Err(e) = native::set_autostart(settings.auto_start) {
                warning = Some(format!("开机启动设置失败：{e}"));
            }
            cx.spawn_background(|cancel| {
                if !cancel.is_cancelled() {
                    crate::dictionary::warm_up();
                    offline::warm_up();
                }
                Message::Warmed
            });
        }
        Self {
            settings_window: input.settings_window,
            parent: input.parent.clone(),
            settings_open: false,
            tab: if input.tab.is_empty() {
                "general".into()
            } else {
                input.tab.clone()
            },
            text: input.initial_text.clone(),
            force_online_for_long_text: false,
            pending_provider: None,
            output: None,
            entry: None,
            details_open: false,
            provider_picker_open: false,
            provider_picker_selected: None,
            provider_picker_revision: 0,
            color_scheme: ColorScheme::Light,
            settings,
            history,
            query: String::new(),
            busy: false,
            revision: 0,
            task: None,
            speech_task: None,
            speech_revision: 0,
            speech_error: None,
            phonetics: None,
            phonetics_task: None,
            selection_task: None,
            selection_revision: 0,
            status: warning.clone().unwrap_or_default(),
            error: warning.is_some(),
            input: ElementRef::new(),
        }
    }
    fn update(&mut self, message: Message, cx: &ComponentContext<Self>) {
        match message {
            Message::Text(value) => {
                if self.text != value {
                    self.cancel_selection();
                    self.cancel_speech();
                    self.text = value;
                    let previously_followed_input = self.settings.auto_pair;
                    self.settings.follow_bilingual_input(&self.text);
                    if !previously_followed_input && self.settings.auto_pair {
                        let _ = storage::save_settings(&self.settings);
                    }
                    self.force_online_for_long_text = false;
                    self.pending_provider = None;
                    self.schedule(cx);
                }
            }
            Message::Translate => self.translate(cx),
            Message::Debounced(revision) if revision == self.revision => {
                if self.text.trim().is_empty() {
                    self.output = None;
                    self.entry = None;
                    self.details_open = false;
                    self.error = false;
                    self.status.clear();
                } else if native::is_composing() {
                    self.schedule(cx);
                } else {
                    self.translate(cx);
                }
            }
            Message::Details(open) => self.details_open = open,
            Message::Pronounce(accent) => {
                let Some(text) = self.pronunciation_text().map(str::to_owned) else {
                    return;
                };
                self.cancel_speech();
                let revision = self.speech_revision;
                self.speech_task = Some(cx.spawn_background(move |cancel| {
                    Message::Pronounced(
                        revision,
                        crate::speech::pronounce(&text, accent, &|| cancel.is_cancelled()),
                    )
                }));
            }
            Message::Pronounced(revision, result) if revision == self.speech_revision => {
                self.speech_task = None;
                self.speech_error = result.err();
            }
            Message::PhoneticsReady(revision, result) if revision == self.revision => {
                self.phonetics_task = None;
                self.phonetics = result.ok();
            }
            Message::ColorScheme(scheme) => self.color_scheme = scheme,
            Message::RepairCaret(attempt) => self.repair_caret(cx, attempt),
            Message::RetryCaret(attempt) => {
                cx.spawn_background(move |_| {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    Message::RepairCaret(attempt)
                });
            }
            Message::Finished(revision, result, entry) if revision == self.revision => {
                self.busy = false;
                self.pending_provider = None;
                self.task = None;
                self.entry = entry;
                match result {
                    Ok(mut t) => {
                        self.error = false;
                        if self
                            .output
                            .as_ref()
                            .is_none_or(|old| old.source != t.source)
                        {
                            self.details_open = false;
                        }
                        t.starred = self
                            .history
                            .iter()
                            .any(|old| old.source == t.source && old.to == t.to && old.starred);
                        self.history.retain(|old| {
                            !(old.source == t.source
                                && old.from == t.from
                                && old.to == t.to
                                && old.provider == t.provider)
                        });
                        self.history.insert(0, t.clone());
                        self.history.truncate(100);
                        self.output = Some(t);
                        self.load_phonetics(cx);
                        if let Err(e) = storage::save_history(&self.history) {
                            self.status = format!("记录保存失败：{e}");
                        }
                    }
                    Err(e) => {
                        self.output = None;
                        self.entry = None;
                        self.details_open = false;
                        self.error = true;
                        self.status = e;
                    }
                }
            }
            Message::Dismiss => {
                self.cancel_selection();
                if self.provider_picker_open {
                    self.close_provider_picker();
                } else {
                    self.cancel_speech();
                    self.invalidate();
                    native::hide_popup();
                }
            }
            Message::Show => {
                self.cancel_selection();
                self.close_provider_picker();
                native::show_popup();
                let _ = self.input.request_focus();
                self.repair_caret(cx, 0);
                if self
                    .output
                    .as_ref()
                    .is_some_and(|t| t.source != self.text.trim())
                    || (!self.text.trim().is_empty() && self.output.is_none())
                {
                    self.schedule(cx);
                }
            }
            Message::CaptureSelection => {
                self.cancel_selection();
                let revision = self.selection_revision;
                let request = crate::selection::CaptureRequest::new();
                self.selection_task = Some(cx.spawn_background(move |cancel| {
                    Message::SelectionCaptured(
                        revision,
                        request.and_then(|request| request.capture(&|| cancel.is_cancelled())),
                    )
                }));
            }
            Message::SelectionCaptured(revision, selected)
                if revision == self.selection_revision =>
            {
                self.selection_task = None;
                if let Some(selected) = selected.filter(|value| !value.trim().is_empty()) {
                    if self.text != selected {
                        self.cancel_speech();
                        self.invalidate();
                        self.text = selected;
                        self.settings.follow_bilingual_input(&self.text);
                        self.force_online_for_long_text = false;
                        self.pending_provider = None;
                    }
                }
                let _ = cx.sender().send(Message::Show);
            }
            Message::Quit => {
                self.cancel_selection();
                self.cancel_speech();
                self.invalidate();
                native::quit();
            }
            Message::Composition(active) => {
                if active {
                    self.invalidate();
                } else {
                    self.schedule(cx);
                }
            }
            Message::Drag => native::drag_popup(),
            Message::OpenSettings => self.open_settings(cx),
            Message::SettingsClosed => {
                self.settings_open = false;
                self.settings = storage::load().0;
            }
            Message::Reload => {
                let settings = storage::load().0;
                let changed = settings.provider != self.settings.provider
                    || settings.credentials != self.settings.credentials
                    || settings.region != self.settings.region
                    || settings.deepl_free != self.settings.deepl_free
                    || settings.auto_offline != self.settings.auto_offline
                    || settings.auto_pair != self.settings.auto_pair;
                self.settings = settings;
                if changed {
                    self.schedule(cx);
                }
            }
            Message::Tab(Some(tab)) => {
                self.tab = tab;
                if self.tab == "history" {
                    self.history = storage::load().1;
                }
            }
            Message::ProviderPicker(open) => {
                if open {
                    self.provider_picker_revision = self.provider_picker_revision.wrapping_add(1);
                    self.provider_picker_selected =
                        Some(self.pending_provider.unwrap_or_else(|| {
                            self.output
                                .as_ref()
                                .map_or(self.settings.provider, |translation| translation.provider)
                        }));
                    self.provider_picker_open = true;
                } else {
                    self.close_provider_picker();
                }
            }
            Message::CloseProviderPicker(revision) if revision == self.provider_picker_revision => {
                self.close_provider_picker()
            }
            Message::Provider(Some(i)) if i < Provider::ALL.len() => {
                let provider = Provider::ALL[i];
                let selected_from_picker = self.provider_picker_open;
                if !self.settings_window
                    && (!provider_configured(&self.settings, provider)
                        || (self.provider_picker_open
                            && self.provider_picker_selected == Some(provider)))
                {
                    return;
                }
                let force_online = self.text.chars().count() > api::ONLINE_TEXT_LIMIT
                    && provider != Provider::Offline;
                let policy_changed = self.force_online_for_long_text != force_online;
                self.force_online_for_long_text = force_online;
                let changed = self.settings.provider != Provider::ALL[i];
                if changed {
                    self.settings.provider = Provider::ALL[i];
                    self.save();
                }
                if !self.settings_window {
                    if self.provider_picker_open {
                        self.provider_picker_selected = Some(provider);
                        self.provider_picker_revision =
                            self.provider_picker_revision.wrapping_add(1);
                        let revision = self.provider_picker_revision;
                        cx.spawn_background(move |_| {
                            std::thread::sleep(std::time::Duration::from_millis(180));
                            Message::CloseProviderPicker(revision)
                        });
                    } else {
                        let _ = self.input.request_focus();
                    }
                    if changed || policy_changed || selected_from_picker {
                        self.pending_provider = Some(provider);
                        self.schedule(cx);
                    }
                }
            }
            Message::Source(Some(i)) if i < LANGUAGES.len() => {
                let (from, to) = self.language_indices();
                if i != from {
                    self.settings.from_index = i;
                    self.settings.to_index = if i == to {
                        if from == 0 {
                            if to == 1 { 2 } else { 1 }
                        } else {
                            from
                        }
                    } else {
                        to
                    };
                    self.settings.auto_pair = false;
                    self.save();
                    self.schedule(cx);
                }
            }
            Message::Target(Some(i)) if i + 1 < LANGUAGES.len() => {
                let (from, to) = self.language_indices();
                if i + 1 != to {
                    self.settings.from_index = if i + 1 == from { to } else { from };
                    self.settings.to_index = i + 1;
                    self.settings.auto_pair = false;
                    self.save();
                    self.schedule(cx);
                }
            }
            Message::Swap => {
                let (from, to) = self.language_indices();
                self.settings.from_index = to;
                self.settings.to_index = if from == 0 {
                    if detect(&self.text) == "zh" { 1 } else { 2 }
                } else {
                    from
                };
                self.settings.auto_pair = false;
                self.save();
                self.schedule(cx);
            }
            Message::AutoPair(value) => {
                if self.settings.auto_pair != value {
                    self.settings.auto_pair = value;
                    self.save();
                }
            }
            Message::AutoStart(value) => {
                if self.settings.auto_start != value {
                    match native::set_autostart(value) {
                        Ok(()) => {
                            self.settings.auto_start = value;
                            self.save();
                        }
                        Err(e) => {
                            self.status = format!("开机启动设置失败：{e}");
                            self.error = true;
                        }
                    }
                }
            }
            Message::SetId(id) => {
                if let Some(c) = self
                    .settings
                    .credentials
                    .get_mut(self.settings.provider.index())
                {
                    c.id = id;
                }
            }
            Message::SetKey(key) => {
                if let Some(c) = self
                    .settings
                    .credentials
                    .get_mut(self.settings.provider.index())
                {
                    c.key = key;
                }
            }
            Message::Region(region) => self.settings.region = region,
            Message::AutoOffline(value) => {
                if self.settings.auto_offline != value {
                    self.settings.auto_offline = value;
                    self.save();
                }
            }
            Message::DeepLFree(value) => {
                if self.settings.deepl_free != value {
                    self.settings.deepl_free = value;
                    self.save();
                }
            }
            Message::Theme(Some(i)) if i < 3 && self.settings.theme != i => {
                self.settings.theme = i;
                self.save();
            }
            Message::Save => self.save(),
            Message::Search(query) => self.query = query,
            Message::Restore(i) => {
                if let Some(t) = self.history.get(i).cloned() {
                    if let Some(parent) = &self.parent {
                        let _ = parent.send(Message::Text(t.source));
                        let _ = parent.send(Message::Translate);
                        let _ = cx.window().request_close();
                    }
                }
            }
            Message::Star(i) => {
                if let Some(t) = self.history.get_mut(i) {
                    t.starred = !t.starred;
                    let _ = storage::save_history(&self.history);
                }
            }
            _ => {}
        }
    }
    fn view(&self, _: &WindowInput, cx: &mut ViewContext<Self>) -> View {
        if self.settings_window {
            self.settings_view(cx)
        } else {
            self.popup(cx)
        }
    }
}
impl Drop for Translator {
    fn drop(&mut self) {
        self.cancel_selection();
        if let Some(task) = self.phonetics_task.take() {
            task.cancel();
        }
        if !self.settings_window {
            self.cancel_speech();
        }
        if let Some(task) = self.task.take() {
            task.cancel();
        }
        if let Some(parent) = &self.parent {
            let _ = parent.send(Message::SettingsClosed);
        }
    }
}
