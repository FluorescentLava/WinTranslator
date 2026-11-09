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
    pub initial_configuration: Option<(Settings, Vec<Translation>, Option<String>)>,
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
    ChineseDefinitionsReady(u64, Option<crate::dictionary::Entry>),
    Details(bool),
    PopupFrame(native::PopupFrame),
    Pronounce(crate::speech::Accent),
    Pronounced(u64, Result<(), String>),
    LocalPhoneticReady(u64, String, String),
    PhoneticsReady(u64, String, Result<crate::speech::Phonetics, String>),
    ColorScheme(ColorScheme),
    RepairCaret(u8),
    RetryCaret(u8),
    Dismiss,
    Drag,
    Show,
    PopupActivated(u64),
    PopupFocusReady(u64),
    FocusPopupInput(u64, u8),
    InputFocusResult(u64, u8, bool),
    PopupDismissed(u64),
    PopupHidden(u64),
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
    SetId(Provider, String),
    SetKey(Provider, String),
    Region(String),
    AutoOffline(bool),
    DeepLPlan(Option<usize>),
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
    pending_chinese_entry: Option<crate::dictionary::Entry>,
    definitions_task: Option<ComponentTask>,
    details_open: bool,
    popup_frame: Option<native::PopupFrame>,
    popup_details_reveal: f64,
    popup_details_from: f64,
    popup_committed_reveal: std::rc::Rc<std::cell::Cell<f64>>,
    provider_picker_open: bool,
    provider_picker_selected: Option<Provider>,
    provider_picker_revision: u64,
    color_scheme: ColorScheme,
    settings: Settings,
    settings_load_warning: Option<String>,
    history: Vec<Translation>,
    query: String,
    busy: bool,
    revision: u64,
    task: Option<ComponentTask>,
    speech_task: Option<ComponentTask>,
    speech_revision: u64,
    speech_error: Option<String>,
    phonetics_word: String,
    local_phonetic: String,
    phonetics: Option<crate::speech::Phonetics>,
    phonetics_task: Option<ComponentTask>,
    selection_task: Option<ComponentTask>,
    selection_revision: u64,
    status: String,
    error: bool,
    input: ElementRef<TextBox>,
    provider_button: ElementRef<Button>,
    popup_surface: ElementRef<Border>,
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
fn display_senses(
    entry: &crate::dictionary::Entry,
    primary: &str,
) -> Vec<crate::dictionary::Sense> {
    if entry.direction == crate::dictionary::LookupDirection::ChineseToEnglish {
        let primary = meaning_key(primary);
        return entry
            .senses
            .iter()
            .filter(|sense| meaning_key(&sense.definition) != primary)
            .cloned()
            .collect();
    }
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
    if entry.parts_of_speech.is_empty() {
        "词性未收录".into()
    } else {
        entry.parts_of_speech.join(" · ")
    }
}

fn merge_chinese_entries(
    local: Option<crate::dictionary::Entry>,
    online: Option<crate::dictionary::Entry>,
) -> Option<crate::dictionary::Entry> {
    let Some(online) = online else {
        return local;
    };
    if let Some(local) = local {
        if local.direction != online.direction || local.word != online.word {
            return Some(local);
        }
    }
    // Exact Chinese-English entries take priority over reverse-index matches,
    // which can also contain inflected words or borrowings from other languages.
    Some(online)
}
fn at(view: impl Into<View>, row: i32, column: i32) -> View {
    Grid::new()
        .grid_row(row)
        .grid_column(column)
        .children((view.into(),))
}
// Native Fluent vector glyphs use their intrinsic size and centered layout.
const TRANSLATE: &str = "\u{E8C1}";
const SETTINGS: &str = "\u{E713}";
const HISTORY: &str = "\u{E81C}";
const POPUP_WIDTH: f64 = 480.0;
const POPUP_CONTENT_WIDTH: f64 = POPUP_WIDTH - 32.0;
fn icon(path: &str) -> FontIcon {
    FontIcon::new().glyph(path)
}
fn speaker_icon() -> View {
    Viewbox::new()
        .width(14.0)
        .height(14.0)
        .stretch(windows_reactor::Stretch::Uniform)
        .opacity(0.7)
        .horizontal_alignment(HorizontalAlignment::Center)
        .vertical_alignment(Center)
        // Optical alignment: shift the glyph left 1 DIP and down 2 DIPs.
        .margin(Thickness::new(-1.0, 2.0, 1.0, -2.0))
        .slot(
            ViewboxSlot::Child,
            icon("\u{E994}")
                .width(20.0)
                .height(20.0)
                .horizontal_alignment(HorizontalAlignment::Center)
                .vertical_alignment(Center),
        )
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
        .width(32.0)
        .height(32.0)
        .min_width(0.0)
        .min_height(32.0)
        .padding(6.0)
        .horizontal_content_alignment(HorizontalAlignment::Center)
        .vertical_content_alignment(Center)
        .automation_name(name)
        .on_click(cx.message(message))
        .content(
            Viewbox::new()
                .width(16.0)
                .height(16.0)
                .stretch(windows_reactor::Stretch::Uniform)
                .slot(ViewboxSlot::Child, icon(path)),
        )
        .tooltip(name)
}
fn swap_button(cx: &ViewContext<Translator>, dark: bool) -> View {
    let foreground = if dark {
        Color::rgb(255, 255, 255)
    } else {
        Color::rgb(0, 0, 0)
    };
    Button::new()
        .style(ButtonStyle::Subtle)
        .width(32.0)
        .height(32.0)
        .min_width(0.0)
        .min_height(32.0)
        .padding(5.0)
        .resource_overrides(
            [
                "TextFillColorPrimaryBrush",
                "TextFillColorSecondaryBrush",
                "ButtonForeground",
                "ButtonForegroundPointerOver",
                "ButtonForegroundPressed",
            ]
            .into_iter()
            .fold(ResourceOverrides::new(), |resources, key| {
                resources.set(key, ResourceValue::Brush(foreground))
            }),
        )
        .horizontal_content_alignment(HorizontalAlignment::Center)
        .vertical_content_alignment(Center)
        .automation_name("交换语言")
        .on_click(cx.message(Message::Swap))
        .content(
            Viewbox::new()
                .width(16.0)
                .height(16.0)
                .stretch(windows_reactor::Stretch::Uniform)
                .horizontal_alignment(HorizontalAlignment::Center)
                .vertical_alignment(Center)
                .slot(
                    ViewboxSlot::Child,
                    Border::new().width(24.0).height(24.0).content(
                        PathIcon::new()
                            .data(include_str!("../assets/icons/arrow-swap.path"))
                            .width(24.0)
                            .height(24.0)
                            .opacity(1.0),
                    ),
                ),
        )
        .tooltip("交换语言")
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
        let word = self
            .pronunciation_text()
            .filter(|text| crate::speech::supports_word_phonetics(text))
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if word != self.phonetics_word {
            if let Some(task) = self.phonetics_task.take() {
                task.cancel();
            }
            self.phonetics_word = word.clone();
            self.phonetics = None;
            self.local_phonetic.clear();
        }
        if word.is_empty()
            || self.phonetics_task.is_some()
            || self
                .phonetics
                .as_ref()
                .is_some_and(|value| !value.british.is_empty() && !value.american.is_empty())
        {
            return;
        }
        let revision = self.revision;
        self.phonetics_task = Some(cx.spawn_background(move |cancel| {
            // The English result of a Chinese lookup needs its own local IPA;
            // the translation entry contains Chinese headwords in that case.
            let local = if cancel.is_cancelled() {
                String::new()
            } else {
                crate::dictionary::lookup(&word)
                    .map(|entry| entry.phonetic)
                    .unwrap_or_default()
            };
            Message::LocalPhoneticReady(revision, word, local)
        }));
    }

    fn pronunciation_controls(&self, cx: &ViewContext<Self>) -> View {
        let Some(pronunciation) = self.pronunciation_text() else {
            return View::empty();
        };
        if !crate::speech::supports_word_phonetics(pronunciation) {
            return Button::new()
                .style(ButtonStyle::Subtle)
                .width(32.0)
                .height(32.0)
                .min_width(0.0)
                .min_height(32.0)
                .padding(0.0)
                .horizontal_alignment(HorizontalAlignment::Left)
                .vertical_alignment(Center)
                .horizontal_content_alignment(HorizontalAlignment::Center)
                .vertical_content_alignment(Center)
                .automation_name("朗读")
                .on_click(cx.message(Message::Pronounce(crate::speech::Accent::American)))
                .content(speaker_icon())
                .tooltip("朗读")
                .into();
        }
        let phonetics = self
            .phonetics
            .as_ref()
            .filter(|_| self.phonetics_word.eq_ignore_ascii_case(pronunciation));
        let common_phonetic = if phonetics
            .is_none_or(|value| value.british.is_empty() && value.american.is_empty())
        {
            let local = self
                .phonetics_word
                .eq_ignore_ascii_case(pronunciation)
                .then_some(self.local_phonetic.as_str())
                .filter(|value| !value.trim().is_empty());
            local
                .or_else(|| {
                    self.entry
                        .as_ref()
                        .filter(|entry| {
                            entry.direction == crate::dictionary::LookupDirection::EnglishToChinese
                                && entry.word.eq_ignore_ascii_case(pronunciation)
                        })
                        .map(|entry| entry.phonetic.as_str())
                })
                .map(|value| value.trim().trim_matches('/').trim())
                .filter(|value| !value.is_empty())
        } else {
            None
        };
        let accents = Grid::new()
            .columns([Auto, Auto])
            .horizontal_alignment(HorizontalAlignment::Left)
            .vertical_alignment(Center)
            .column_spacing(8.0)
            .keyed_children(
                [
                    (crate::speech::Accent::British, "英", "英式发音"),
                    (crate::speech::Accent::American, "美", "美式发音"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (accent, label, name))| {
                    let phonetic = phonetics.map_or("", |value| match accent {
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
                            Grid::new()
                                .columns([Auto, Auto, Auto])
                                .horizontal_alignment(HorizontalAlignment::Left)
                                .vertical_alignment(Center)
                                .column_spacing(4.0)
                                .children((
                                    text(label, 14.0)
                                        .opacity(0.65)
                                        .is_hit_test_visible(false)
                                        .vertical_alignment(Center),
                                    text(&phonetic, 14.0)
                                        .max_width(88.0)
                                        .text_wrapping(TextWrapping::NoWrap)
                                        .text_trimming(TextTrimming::CharacterEllipsis)
                                        .opacity(0.65)
                                        .is_hit_test_visible(false)
                                        .vertical_alignment(Center)
                                        .grid_column(1)
                                        .tooltip(&phonetic),
                                    Button::new()
                                        .style(ButtonStyle::Subtle)
                                        .width(28.0)
                                        .height(28.0)
                                        .min_width(0.0)
                                        .min_height(28.0)
                                        .padding(0.0)
                                        .vertical_alignment(Center)
                                        .horizontal_content_alignment(HorizontalAlignment::Center)
                                        .vertical_content_alignment(Center)
                                        .automation_name(name)
                                        .on_click(cx.message(Message::Pronounce(accent)))
                                        .grid_column(2)
                                        .content(speaker_icon())
                                        .tooltip(format!("{name} {phonetic}")),
                                )),
                            0,
                            index as i32,
                        ),
                    )
                }),
            );
        Grid::new()
            .columns([Auto, Auto])
            .horizontal_alignment(HorizontalAlignment::Left)
            .children((
                if let Some(phonetic) = common_phonetic {
                    text(format!("/{phonetic}/"), 14.0)
                        .max_width(88.0)
                        .text_wrapping(TextWrapping::NoWrap)
                        .text_trimming(TextTrimming::CharacterEllipsis)
                        .opacity(0.65)
                        .is_hit_test_visible(false)
                        .vertical_alignment(Center)
                        .margin(Thickness::new(0.0, 0.0, 8.0, 0.0))
                        .automation_name("词典音标，未区分口音")
                        .tooltip(format!("词典音标 /{phonetic}/（未区分口音）"))
                        .into()
                } else {
                    View::empty()
                },
                at(accents, 0, 1),
            ))
    }

    fn invalidate(&mut self) {
        self.pending_chinese_entry = None;
        if let Some(task) = self.definitions_task.take() {
            task.cancel();
        }
        self.revision += 1;
        self.busy = false;
        // Hiding cancels work without discarding the displayed word's IPA.
        // A different pronunciation word is reset by load_phonetics instead.
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
                self.settings_load_warning = None;
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
        if to == "en" && (from == "zh" || (from == "auto" && detect(&text) == "zh")) {
            let query = text.clone();
            self.definitions_task = Some(cx.spawn_background(move |cancel| {
                Message::ChineseDefinitionsReady(
                    revision,
                    crate::chinese_dictionary::lookup(&query, &|| cancel.is_cancelled()),
                )
            }));
        }
        self.task = Some(cx.spawn_background(move |cancel| {
            let actual_from = if from == "auto" { detect(&text) } else { &from };
            let entry = if actual_from == "en" && to == "zh" {
                crate::dictionary::lookup(&text)
            } else if actual_from == "zh" && to == "en" {
                crate::dictionary::lookup_chinese(&text)
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
            .width(if compact { 224.0 } else { 420.0 })
            .height(if compact { 224.0 } else { 144.0 })
            .min_width(0.0)
            .min_height(0.0)
            .horizontal_alignment(HorizontalAlignment::Left)
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .open_pane_length(if compact { 224.0 } else { 420.0 })
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
                        let missing_status = if self.settings_load_warning.is_some() {
                            "读取失败"
                        } else {
                            "未配置"
                        };
                        let label = if compact && !configured {
                            format!("{} · {missing_status}", provider.name())
                        } else {
                            provider.name().to_owned()
                        };
                        let status = match provider {
                            Provider::Offline if offline::ready() => "已就绪",
                            Provider::Offline => "未就绪",
                            Provider::MyMemory => "免密钥试用",
                            _ if configured => "已配置",
                            _ => missing_status,
                        };
                        let content = if compact {
                            StackPanel::new()
                                .orientation(Orientation::Horizontal)
                                .spacing(8.0)
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
                        let item = NavigationViewItem::new()
                            .tag(index.to_string())
                            .is_selected(provider == selected)
                            .selects_on_invoked(selectable)
                            .opacity(if selectable { 1.0 } else { 0.45 })
                            .automation_name(if compact {
                                label
                            } else {
                                format!("{}，{status}", provider.name())
                            });
                        let item = if compact {
                            item.height(36.0).min_height(36.0).margin(0.0)
                        } else {
                            item
                        };
                        (index, item.slot(NavigationViewItemSlot::Content, content))
                    }),
            )])
    }
    fn close_provider_picker(&mut self) {
        let _ = self.provider_button.request_hide_flyout();
        self.provider_picker_open = false;
        self.provider_picker_selected = None;
        self.provider_picker_revision = self.provider_picker_revision.wrapping_add(1);
        if !self.settings_window && native::popup_is_foreground() {
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
            initial_configuration: None,
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
    fn focus_popup_input(&self, cx: &ComponentContext<Self>, revision: u64, attempt: u8) {
        if !native::popup_show_animation_is_current(revision)
            || !native::prepare_popup_input_focus()
        {
            return;
        }
        let sender = cx.sender();
        let accepted = self.input.request_focus_result(move |result| {
            if let Err(error) = &result {
                storage::diagnostic(serde_json::json!({
                    "event":"popup_input_focus_error", "attempt":attempt,
                    "error":format!("{error:?}"),
                }));
            }
            let _ = sender.send(Message::InputFocusResult(
                revision,
                attempt,
                matches!(result, Ok(true)),
            ));
        });
        if !accepted {
            let _ = cx
                .sender()
                .send(Message::InputFocusResult(revision, attempt, false));
        }
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
            .and_then(|translation| {
                self.entry
                    .as_ref()
                    .map(|entry| display_senses(entry, &translation.target))
            })
            .unwrap_or_default();
        let details_open = self.details_open && !dictionary_senses.is_empty();
        let details_reveal = if dictionary_senses.is_empty() {
            0.0
        } else {
            self.popup_details_reveal.clamp(0.0, 1.0)
        };
        let details_mounted = details_open || details_reveal > 0.0;
        let is_word = self.output.as_ref().is_some_and(|translation| {
            !translation.source.trim().is_empty()
                && translation
                    .source
                    .trim()
                    .chars()
                    .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '\'')
        });
        let dictionary_result = self.entry.is_some() || is_word;
        let inline_pronunciation = dictionary_result && self.pronunciation_text().is_some();
        let summary_target_width = if inline_pronunciation {
            96.0
        } else {
            POPUP_CONTENT_WIDTH - 56.0
        };
        let long_input = self.text.chars().count() > 80
            || visual_lines(&self.text, 23.0, POPUP_CONTENT_WIDTH - 12.0) > 2
            || self
                .text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
                > 1;
        let input_font = if long_input { 15.0 } else { 23.0 };
        let input_line_height = if long_input { 21.0 } else { 32.0 };
        let input_height = (visual_lines(&self.text, input_font, POPUP_CONTENT_WIDTH - 12.0)
            as f64
            * input_line_height
            + 12.0)
            .clamp(60.0, 180.0);
        let fixed_height = 107.0 + input_height;
        let available_result = (480.0 - fixed_height).max(80.0);
        let frame_height = self.popup_frame.map_or(172.0, |frame| frame.height);
        let input_viewport_height = input_height.min((frame_height - 107.0).max(0.0));
        let frame_result_height = (frame_height - 107.0 - input_viewport_height).max(0.0);
        let result_font = self.output.as_ref().map_or(23.0, |translation| {
            if long_input
                || translation.target.chars().count() > 80
                || visual_lines(&translation.target, 23.0, POPUP_CONTENT_WIDTH - 56.0) > 2
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
        // Dictionary summaries always reserve the same row for the translation
        // and pronunciation. Loading IPA must not switch the card's layout.
        let pronunciation_height = if self.pronunciation_text().is_some() && !dictionary_result {
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
            visual_lines(notice, 12.0, POPUP_CONTENT_WIDTH - 32.0) as f64 * 18.0 + 6.0
        });
        let metadata = self
            .entry
            .as_ref()
            .map(dictionary_metadata)
            .filter(|value| !value.is_empty())
            .or_else(|| is_word.then(|| "词性未收录".into()));
        let mut target_max_height = available_result;
        let mut details_height = 0.0;
        let mut details_viewport_height = 0.0;
        let (result_height, rendered_result_height) = if let Some(translation) = &self.output {
            if dictionary_result {
                let metadata_height = metadata.as_ref().map_or(0.0, |value| {
                    visual_lines(value, 14.0, POPUP_CONTENT_WIDTH - 56.0) as f64 * 20.0 + 6.0
                });
                let header_fixed = 28.0 + metadata_height + pronunciation_height;
                let estimated_target =
                    visual_lines(&translation.target, result_font, summary_target_width) as f64
                        * result_line_height;
                let closed_target =
                    estimated_target.min((available_result - header_fixed).max(result_line_height));
                let open_target = estimated_target
                    .min((available_result - header_fixed - 72.0).max(result_line_height));
                let open_header = header_fixed + open_target;
                let meanings_height = dictionary_senses
                    .iter()
                    .map(|sense| {
                        visual_lines(&sense.definition, 15.0, POPUP_CONTENT_WIDTH - 118.0) as f64
                            * 21.0
                    })
                    .sum::<f64>()
                    + dictionary_senses.len().saturating_sub(1) as f64 * 12.0;
                details_height =
                    (meanings_height + 25.0).min((available_result - open_header).max(25.0));
                details_viewport_height = (details_height - 25.0).max(0.0);
                // The target is independent of animation progress. Both the
                // summary budget and detail reveal use the same eased frame.
                target_max_height = closed_target + (open_target - closed_target) * details_reveal;
                let target_height = if details_open {
                    open_header + details_height
                } else {
                    header_fixed + closed_target
                };
                (
                    target_height,
                    header_fixed + target_max_height + details_height * details_reveal,
                )
            } else {
                let word_metadata = if is_word { 26.0 } else { 0.0 };
                let height = (30.0
                    + notice_height
                    + pronunciation_height
                    + word_metadata
                    + visual_lines(&translation.target, result_font, POPUP_CONTENT_WIDTH - 32.0)
                        as f64
                        * result_line_height)
                    .min(available_result);
                (height, height)
            }
        } else if self.error {
            let height =
                (visual_lines(&self.status, 12.0, POPUP_CONTENT_WIDTH - 12.0).min(4) as f64 * 18.0
                    + 4.0)
                    .min(available_result);
            (height, height)
        } else {
            (0.0, 0.0)
        };
        let rendered_result_height = rendered_result_height.min(frame_result_height);
        let details_reveal_height = details_height * details_reveal;
        let natural_height = if expanded || self.error {
            (fixed_height + result_height).clamp(172.0, 480.0)
        } else {
            fixed_height.max(172.0).min(480.0)
        };
        let height = natural_height;
        let transition_key =
            u64::from(details_open) | (u64::from(expanded) << 2) | (u64::from(self.error) << 3);
        cx.window_title("WinTranslater");
        cx.window_visuals(
            WindowVisuals::new()
                .client_size(
                    POPUP_WIDTH + 2.0 * f64::from(native::POPUP_SHADOW_INSET),
                    480.0 + 2.0 * f64::from(native::POPUP_SHADOW_INSET),
                )
                .backdrop(WindowBackdrop::MicaCard {
                    inset: native::POPUP_SHADOW_INSET,
                })
                .theme(theme(&self.settings)),
        );
        let dismiss = cx.message(Message::Dismiss);
        cx.use_effect(
            "popup-target",
            (transition_key, height.to_bits()),
            move || {
                native::configure_popup_with_key(POPUP_WIDTH, height, transition_key);
                native::escape_handler(move || {
                    let _ = dismiss.call(());
                });
                None
            },
        );
        let frame = self.popup_frame;
        let reveal = self.popup_details_reveal;
        let committed_reveal = self.popup_committed_reveal.clone();
        cx.use_effect(
            "popup-frame-commit",
            frame.map(|frame| frame.serial),
            move || {
                if let Some(frame) = frame
                    && native::popup_frame_is_current(frame)
                {
                    native::commit_popup_frame(frame);
                    committed_reveal.set(reveal);
                }
                None
            },
        );
        cx.use_effect("popup-initial-show", (), move || {
            // The first WinUI Activate can inherit SW_HIDE from silent startup.
            // Use the same committed-frame show/activation path as later opens.
            native::show_popup();
            None
        });
        let caret_sender = cx.sender();
        cx.use_effect("input-caret", (dark, self.text.clone()), move || {
            let _ = caret_sender.send(Message::RepairCaret(0));
            None
        });
        let (from, to) = self.language_indices();
        let languages = Grid::new()
            .columns([Auto, Auto, Auto, Auto, Star(1.0)])
            .column_spacing(8.0)
            .children((
                Border::new()
                    .width(32.0)
                    .height(32.0)
                    .vertical_alignment(Center)
                    .content(
                        Viewbox::new()
                            .width(20.0)
                            .height(20.0)
                            .stretch(windows_reactor::Stretch::Uniform)
                            .horizontal_alignment(HorizontalAlignment::Center)
                            .vertical_alignment(Center)
                            .slot(ViewboxSlot::Child, icon(TRANSLATE)),
                    ),
                ComboBox::new()
                    .items_source(LANGUAGES.iter().map(popup_language))
                    .selected_index(from)
                    .width(108.0)
                    .height(32.0)
                    .min_width(0.0)
                    .min_height(32.0)
                    .vertical_alignment(Center)
                    .automation_name("源语言")
                    .on_selection_changed(cx.callback(Message::Source))
                    .grid_column(1),
                at(swap_button(cx, dark), 0, 2),
                ComboBox::new()
                    .items_source(LANGUAGES.iter().skip(1).map(popup_language))
                    .selected_index(to - 1)
                    .width(108.0)
                    .height(32.0)
                    .min_width(0.0)
                    .min_height(32.0)
                    .vertical_alignment(Center)
                    .automation_name("目标语言")
                    .on_selection_changed(cx.callback(Message::Target))
                    .grid_column(3),
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
            .height(input_viewport_height)
            .min_height(0.0)
            .max_height(180.0)
            .horizontal_alignment(Stretch)
            .automation_name("输入单词或文本")
            .on_text_changed(cx.callback(Message::Text));
        let result: View = if let Some(translation) = &self.output {
            if dictionary_result {
                let has_details = !dictionary_senses.is_empty();
                let details_label = if details_open {
                    "收起词典释义"
                } else {
                    "展开词典释义"
                };
                let chevron = if details_open { "\u{E70E}" } else { "\u{E70D}" };
                // The native details button is a sibling below the visuals.
                // Text passes hit testing through to it; each speaker keeps
                // its own native button without nesting interactive controls.
                let header_content = Grid::new()
                    .rows([Auto, Auto])
                    .columns([Auto, Star(1.0), Auto])
                    .row_spacing(6.0)
                    .column_spacing(12.0)
                    .margin(12.0)
                    .children((
                        if let Some(value) = &metadata {
                            text(value, 14.0)
                                .opacity(0.64)
                                .is_hit_test_visible(false)
                                .grid_column_span(3)
                                .into()
                        } else {
                            View::empty()
                        },
                        ScrollViewer::new()
                            .max_width(summary_target_width)
                            .max_height(target_max_height)
                            .horizontal_alignment(HorizontalAlignment::Left)
                            .is_hit_test_visible(false)
                            .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                            .vertical_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                            .grid_row(1)
                            .content(text(&translation.target, result_font)),
                        if inline_pronunciation {
                            Border::new()
                                .horizontal_alignment(HorizontalAlignment::Left)
                                .vertical_alignment(Center)
                                .grid_row(1)
                                .grid_column(1)
                                .content(self.pronunciation_controls(cx))
                        } else {
                            View::empty()
                        },
                        if has_details {
                            icon(chevron)
                                .opacity(0.6)
                                .is_hit_test_visible(false)
                                .vertical_alignment(Center)
                                .grid_row(1)
                                .grid_column(2)
                                .into()
                        } else {
                            View::empty()
                        },
                    ));
                let header = Grid::new().children((
                    if has_details {
                        Button::new()
                            .style(ButtonStyle::Subtle)
                            .min_width(0.0)
                            .min_height(0.0)
                            .horizontal_alignment(Stretch)
                            .vertical_alignment(VerticalAlignment::Stretch)
                            .horizontal_content_alignment(Stretch)
                            .padding(0.0)
                            .automation_name(details_label)
                            .on_click(cx.message(Message::Details(!details_open)))
                            .into()
                    } else {
                        View::empty()
                    },
                    header_content,
                ));
                let details: View = if details_mounted {
                    let senses = StackPanel::new().spacing(12.0).keyed_children(
                        dictionary_senses.iter().enumerate().map(|(index, sense)| {
                            (
                                index,
                                Grid::new()
                                    .columns([GridLength::Pixel(60.0), Star(1.0)])
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
                    // Keep the body measured at its full size while revealing
                    // only the viewport belonging to the current native frame.
                    ScrollViewer::new()
                        .height(details_reveal_height)
                        .max_height(details_reveal_height)
                        .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                        .vertical_scroll_bar_visibility(ScrollBarVisibility::Disabled)
                        .content(
                            Border::new()
                                .height(details_height)
                                .vertical_alignment(VerticalAlignment::Top)
                                .border_brush(ThemeBrush::CardStroke)
                                .border_thickness(Thickness::new(0.0, 1.0, 0.0, 0.0))
                                .padding(12.0)
                                .content(
                                    ScrollViewer::new()
                                        .height(details_viewport_height)
                                        .horizontal_scroll_bar_visibility(
                                            ScrollBarVisibility::Disabled,
                                        )
                                        .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                                        .content(
                                            Border::new()
                                                .padding(Thickness::new(0.0, 0.0, 10.0, 0.0))
                                                .content(senses),
                                        ),
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
                    .max_height(rendered_result_height)
                    .vertical_alignment(VerticalAlignment::Top)
                    .content(
                        Grid::new()
                            .rows([Auto, Auto])
                            .children((header, at(details, 1, 0))),
                    )
            } else {
                Border::new()
                    .background(ThemeBrush::CardBackground)
                    .border_brush(ThemeBrush::CardStroke)
                    .border_thickness(1.0)
                    .corner_radius(8.0)
                    .padding(14.0)
                    .max_height(rendered_result_height)
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
                                    (rendered_result_height
                                        - 30.0
                                        - notice_height
                                        - pronunciation_height)
                                        .max(0.0),
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
                .max_height(rendered_result_height)
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
        let result = ScrollViewer::new()
            .height(frame_result_height)
            .max_height(frame_result_height)
            .horizontal_scroll_bar_visibility(ScrollBarVisibility::Disabled)
            .vertical_scroll_bar_visibility(ScrollBarVisibility::Disabled)
            .vertical_alignment(VerticalAlignment::Top)
            .content(result);
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
        let closed = cx.message(Message::ProviderPicker(false));
        let state = Button::new()
            .element_ref(&self.provider_button)
            .style(ButtonStyle::Subtle)
            .height(32.0)
            .min_height(32.0)
            .padding(Thickness::new(6.0, 4.0, 6.0, 4.0))
            .horizontal_alignment(HorizontalAlignment::Left)
            .automation_name("选择翻译服务")
            .on_click(cx.message(Message::ProviderPicker(true)))
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
            .flyout_with(
                Flyout::rich(self.provider_choices(
                    self.provider_picker_selected.unwrap_or(provider),
                    true,
                    cx,
                ))
                .placement(FlyoutPlacement::TopEdgeAlignedLeft)
                .constrain_to_root_bounds(false)
                .padding(0.0)
                .min_width(0.0)
                .min_height(0.0)
                .on_closed(move || {
                    let _ = closed.call(());
                }),
            );
        let footer = Grid::new()
            .columns([Auto, Star(1.0), Auto])
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
                at(
                    icon_button(SETTINGS, "设置", Message::OpenSettings, cx),
                    0,
                    2,
                ),
            ));
        let surface = Border::new()
            .element_ref(&self.popup_surface)
            .opacity(0.0)
            .height(frame_height)
            .vertical_alignment(VerticalAlignment::Bottom)
            .padding(Thickness::new(16.0, 12.0, 16.0, 10.0))
            .on_pointer_pressed(cx.callback(|point: PointerEventInfo| {
                if point.y < 8.0 {
                    Message::Drag
                } else {
                    Message::Warmed
                }
            }))
            .content(
                Grid::new().children((
                    Grid::new()
                        .rows([
                            GridLength::Pixel(32.0),
                            GridLength::Pixel(input_viewport_height),
                            Star(1.0),
                        ])
                        .row_spacing(7.0)
                        .margin(Thickness::new(0.0, 0.0, 0.0, 39.0))
                        .children((languages, at(input, 1, 0), at(result, 2, 0))),
                    Border::new()
                        .vertical_alignment(VerticalAlignment::Bottom)
                        .content(footer),
                )),
            );
        surface.into()
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
                            .on_text_changed(
                                cx.callback(move |value| Message::SetId(provider, value)),
                            ),
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
                        .on_password_changed(
                            cx.callback(move |value| Message::SetKey(provider, value)),
                        ),
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
            Provider::DeepL => field(
                "免费计划",
                ComboBox::new()
                    .items_source([
                        "Developer · 总量 100 万",
                        "API Free 旧版 · 每月 50 万",
                        "停用付费计划",
                    ])
                    .selected_index(match self.settings.effective_deepl_plan() {
                        DeepLPlan::Developer => 0,
                        DeepLPlan::LegacyFree => 1,
                        _ => 2,
                    })
                    .on_selection_changed(cx.callback(Message::DeepLPlan)),
            ),
            _ => View::empty(),
        };
        let routes = crate::routing::status(&self.settings);
        let route_rows =
            StackPanel::new()
                .spacing(12.0)
                .keyed_children(routes.into_iter().enumerate().map(|(index, route)| {
                    let note = if !route.configured {
                        if self.settings_load_warning.is_some() {
                            "凭据读取失败".to_owned()
                        } else {
                            "未配置".to_owned()
                        }
                    } else if !route.eligible {
                        format!("未启用 · {}", route.message)
                    } else if route.cooldown_seconds > 0 {
                        format!("暂不可用 · {}", route.message)
                    } else if route.remaining_characters.is_none() {
                        route.message.clone()
                    } else {
                        String::new()
                    };
                    let description = if route.allowance_characters > 0 {
                        let period = match route.allowance_period {
                            crate::routing::AllowancePeriod::Lifetime => "累计",
                            period => period.label(),
                        };
                        let used = route
                            .remaining_characters
                            .map(|remaining| {
                                route
                                    .allowance_characters
                                    .saturating_sub(remaining)
                                    .to_string()
                            })
                            .unwrap_or_else(|| "—".to_owned());
                        let mut description =
                            format!("{period}已用 {used} / {} 字符", route.allowance_characters);
                        if let Some(expires_at) = route.expires_at {
                            description.push_str(&format!(
                                " · 到期 {}",
                                expires_at
                                    .with_timezone(&chrono::Local)
                                    .format("%Y-%m-%d %H:%M")
                            ));
                        }
                        if !note.is_empty() {
                            description.push_str(&format!(" · {note}"));
                        }
                        description
                    } else {
                        note
                    };
                    let progress: View = match route.remaining_characters {
                        Some(remaining) if route.allowance_characters > 0 => ProgressBar::new()
                            .minimum(0.0)
                            .maximum(route.allowance_characters as f64)
                            .value(route.allowance_characters.saturating_sub(remaining) as f64)
                            .is_indeterminate(false)
                            .show_paused(!route.eligible || route.cooldown_seconds > 0)
                            .height(4.0)
                            .automation_name(format!("{}，{description}", route.label))
                            .into(),
                        _ => View::empty(),
                    };
                    (
                        index,
                        StackPanel::new().spacing(6.0).children((
                            Grid::new()
                                .columns([Star(1.0), Auto])
                                .column_spacing(12.0)
                                .children((
                                    text(route.label, 13.0).vertical_alignment(Center),
                                    muted(description)
                                        .vertical_alignment(Center)
                                        .grid_column(1),
                                )),
                            progress,
                        )),
                    )
                }));
        let order = crate::routing::candidates(&self.settings)
            .iter()
            .map(|provider| provider.name())
            .collect::<Vec<_>>()
            .join(" → ");
        let order = if order.is_empty() {
            "手动离线".to_owned()
        } else if self.settings.auto_offline {
            format!("{order} → 本地离线")
        } else {
            order
        };
        StackPanel::new().spacing(10.0).children((
            self.card(
                StackPanel::new().spacing(12.0).children((
                    field("默认引擎", self.provider_choices(provider, false, cx)),
                    StackPanel::new().keyed_children([(provider.index(), credentials)]),
                    extra,
                    Button::new()
                        .style(ButtonStyle::Accent)
                        .on_click(cx.message(Message::Save))
                        .content("保存凭据"),
                )),
            ),
            self.card(StackPanel::new().spacing(10.0).children((
                text("用量明细", 14.0),
                route_rows,
            ))),
            StackPanel::new().spacing(4.0).children((
                text("在线路由", 12.0).opacity(0.75),
                muted(order),
                muted("免费额度不足时自动换服务"),
            )),
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
        cx.window_title("WinTranslater 设置");
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
        let (mut settings, history, mut warning) = input
            .initial_configuration
            .clone()
            .unwrap_or_else(storage::load);
        let settings_load_warning = warning.clone();
        storage::diagnostic(serde_json::json!({
            "event": "settings_loaded",
            "configuration_source": if input.initial_configuration.is_some() {
                "startup_snapshot"
            } else {
                "user_profile"
            },
            "settings_window": input.settings_window,
            "selected_provider": settings.provider,
            "data_directory": storage::data_dir(),
            "load_warning": warning.is_some(),
            "configured_providers": Provider::ALL.into_iter()
                .filter(|provider| provider_configured(&settings, *provider))
                .collect::<Vec<_>>(),
        }));
        if !input.settings_window && !input.initial_text.trim().is_empty() {
            settings.follow_bilingual_input(&input.initial_text);
            cx.spawn_background(|_| Message::Translate);
        }
        if !input.settings_window {
            let sender = cx.sender();
            native::install_handler(move |action| {
                let _ = sender.send(match action {
                    native::Action::Show => Message::Show,
                    native::Action::PopupActivated(revision) => Message::PopupActivated(revision),
                    native::Action::PopupFocusReady(revision) => Message::PopupFocusReady(revision),
                    native::Action::PopupDismissed(revision) => Message::PopupDismissed(revision),
                    native::Action::TranslateSelection => Message::CaptureSelection,
                    native::Action::Settings => Message::OpenSettings,
                    native::Action::Quit => Message::Quit,
                    native::Action::CompositionStarted => Message::Composition(true),
                    native::Action::CompositionEnded => Message::Composition(false),
                });
            });
            let sender = cx.sender();
            native::install_popup_frame_handler(move |frame| {
                let _ = sender.send(Message::PopupFrame(frame));
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
            pending_chinese_entry: None,
            definitions_task: None,
            details_open: false,
            popup_frame: None,
            popup_details_reveal: 0.0,
            popup_details_from: 0.0,
            popup_committed_reveal: std::rc::Rc::new(std::cell::Cell::new(0.0)),
            provider_picker_open: false,
            provider_picker_selected: None,
            provider_picker_revision: 0,
            color_scheme: ColorScheme::Light,
            settings,
            settings_load_warning,
            history,
            query: String::new(),
            busy: false,
            revision: 0,
            task: None,
            speech_task: None,
            speech_revision: 0,
            speech_error: None,
            phonetics_word: String::new(),
            local_phonetic: String::new(),
            phonetics: None,
            phonetics_task: None,
            selection_task: None,
            selection_revision: 0,
            status: warning.clone().unwrap_or_default(),
            error: warning.is_some(),
            input: ElementRef::new(),
            provider_button: ElementRef::new(),
            popup_surface: ElementRef::new(),
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
            Message::PopupFrame(frame) if native::popup_frame_is_current(frame) => {
                if self
                    .popup_frame
                    .is_none_or(|previous| previous.revision != frame.revision)
                {
                    // A reversal starts at the last viewport committed to the
                    // HWND, including when a newer request invalidated a frame.
                    self.popup_details_from = self.popup_committed_reveal.get();
                }
                let target = if frame.transition_key & 1 != 0 {
                    1.0
                } else {
                    0.0
                };
                self.popup_details_reveal =
                    self.popup_details_from + (target - self.popup_details_from) * frame.progress;
                self.popup_frame = Some(frame);
            }
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
            Message::LocalPhoneticReady(revision, word, phonetic)
                if revision == self.revision && word == self.phonetics_word =>
            {
                self.phonetics_task = None;
                if !phonetic.trim().is_empty() {
                    self.local_phonetic = phonetic;
                }
                self.phonetics_task = Some(cx.spawn_background(move |cancel| {
                    let result = crate::speech::phonetics(&word, &|| cancel.is_cancelled());
                    Message::PhoneticsReady(revision, word, result)
                }));
            }
            Message::PhoneticsReady(revision, word, result)
                if revision == self.revision && word == self.phonetics_word =>
            {
                self.phonetics_task = None;
                storage::diagnostic(serde_json::json!({
                    "event": "word_phonetics",
                    "british": result.as_ref().is_ok_and(|value| !value.british.is_empty()),
                    "american": result.as_ref().is_ok_and(|value| !value.american.is_empty()),
                    "error": result.as_ref().err(),
                }));
                if let Ok(value) = result {
                    let current = self.phonetics.get_or_insert_with(Default::default);
                    if !value.british.is_empty() {
                        current.british = value.british;
                    }
                    if !value.american.is_empty() {
                        current.american = value.american;
                    }
                }
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
                self.entry = merge_chinese_entries(entry, self.pending_chinese_entry.take());
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
                        if let Some(task) = self.definitions_task.take() {
                            task.cancel();
                        }
                        self.output = None;
                        self.entry = None;
                        self.details_open = false;
                        self.error = true;
                        self.status = e;
                    }
                }
            }
            Message::ChineseDefinitionsReady(revision, entry) if revision == self.revision => {
                self.definitions_task = None;
                if !self.busy
                    && self
                        .output
                        .as_ref()
                        .is_some_and(|output| output.source.trim() == self.text.trim())
                {
                    self.entry = merge_chinese_entries(self.entry.take(), entry);
                } else {
                    self.pending_chinese_entry = entry;
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
                if self
                    .output
                    .as_ref()
                    .is_some_and(|t| t.source != self.text.trim())
                    || (!self.text.trim().is_empty() && self.output.is_none())
                {
                    self.schedule(cx);
                } else {
                    // A lookup cancelled while hiding must restart even when
                    // the translation itself is still current.
                    self.load_phonetics(cx);
                }
            }
            Message::PopupActivated(revision)
                if native::popup_show_animation_is_current(revision) =>
            {
                let _ = self.popup_surface.request_slide_y(
                    native::popup_slide_distance(),
                    0.0,
                    1.0,
                    1.0,
                    280,
                    |result| {
                        storage::diagnostic(serde_json::json!({
                            "event":"popup_show_animation",
                            "completed":matches!(result, Ok(true)),
                            "cancelled":matches!(result, Ok(false)),
                        }));
                    },
                );
                // The slide request reconnects WinUI's input site first. Focus
                // in that same publication; delayed verification is a fallback.
                self.focus_popup_input(cx, revision, 0);
            }
            Message::PopupFocusReady(revision)
                if native::popup_show_animation_is_current(revision)
                    && native::popup_is_foreground() =>
            {
                self.focus_popup_input(cx, revision, 0);
            }
            Message::FocusPopupInput(revision, attempt) => {
                self.focus_popup_input(cx, revision, attempt);
            }
            Message::InputFocusResult(revision, attempt, focused)
                if native::popup_show_animation_is_current(revision)
                    && native::popup_is_foreground() =>
            {
                let keyboard_focus = native::popup_has_keyboard_focus();
                storage::diagnostic(serde_json::json!({
                    "event":"popup_input_focus", "attempt":attempt,
                    "input_site_verified":focused, "keyboard_focused":keyboard_focus,
                }));
                if focused && keyboard_focus {
                    self.repair_caret(cx, 0);
                } else if attempt < 6 {
                    cx.spawn_background(move |_| {
                        std::thread::sleep(std::time::Duration::from_millis(40));
                        Message::FocusPopupInput(revision, attempt + 1)
                    });
                }
            }
            Message::PopupDismissed(revision)
                if native::popup_hide_animation_is_current(revision) =>
            {
                self.close_provider_picker();
                self.cancel_selection();
                self.cancel_speech();
                self.invalidate();
                let sender = cx.sender();
                let accepted = self.popup_surface.request_slide_y(
                    0.0,
                    native::popup_slide_distance(),
                    1.0,
                    1.0,
                    220,
                    move |result| {
                        storage::diagnostic(serde_json::json!({
                            "event":"popup_hide_animation",
                            "completed":matches!(result, Ok(true)),
                            "cancelled":matches!(result, Ok(false)),
                        }));
                        if !matches!(result, Ok(false)) {
                            let _ = sender.send(Message::PopupHidden(revision));
                        }
                    },
                );
                if !accepted {
                    native::complete_popup_hide(revision);
                }
            }
            Message::PopupHidden(revision) => {
                native::complete_popup_hide(revision);
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
                if let Err(error) = App::quit() {
                    storage::diagnostic(serde_json::json!({
                        "event":"quit_request_failed", "error":error.to_string(),
                    }));
                }
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
                let (settings, _, warning) = storage::load();
                self.settings = settings;
                self.settings_load_warning = warning;
            }
            Message::Reload => {
                let (settings, _, warning) = storage::load();
                self.settings_load_warning = warning;
                let changed = settings.provider != self.settings.provider
                    || settings.credentials != self.settings.credentials
                    || settings.region != self.settings.region
                    || settings.deepl_free != self.settings.deepl_free
                    || settings.deepl_plan != self.settings.deepl_plan
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
                    // Read the shared user profile when opening the picker as well.
                    // Keep the last working credentials if a read fails mid-session.
                    let (settings, _, warning) = storage::load();
                    if warning.is_none() {
                        self.settings.credentials = settings.credentials;
                    }
                    self.settings_load_warning = warning;
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
            Message::SetId(provider, id) => {
                if let Some(c) = self.settings.credentials.get_mut(provider.index()) {
                    c.id = id;
                }
            }
            Message::SetKey(provider, key) => {
                if let Some(c) = self.settings.credentials.get_mut(provider.index()) {
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
            Message::DeepLPlan(Some(index)) if index < 3 => {
                let plan = [DeepLPlan::Developer, DeepLPlan::LegacyFree, DeepLPlan::Paid][index];
                if self.settings.effective_deepl_plan() != plan {
                    self.settings.deepl_plan = plan;
                    self.settings.deepl_free = plan.is_free();
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
