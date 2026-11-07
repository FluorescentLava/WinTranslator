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
}
impl PartialEq for WindowInput {
    fn eq(&self, other: &Self) -> bool {
        self.settings_window == other.settings_window && self.tab == other.tab
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
    ColorScheme(ColorScheme),
    RepairCaret(u8),
    RetryCaret(u8),
    Dismiss,
    Drag,
    Show,
    Quit,
    Composition(bool),
    OpenSettings,
    SettingsClosed,
    Reload,
    Tab(Option<String>),
    Swap,
    Provider(Option<usize>),
    CycleProvider,
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
    output: Option<Translation>,
    entry: Option<crate::dictionary::Entry>,
    details_open: bool,
    color_scheme: ColorScheme,
    settings: Settings,
    history: Vec<Translation>,
    query: String,
    busy: bool,
    revision: u64,
    task: Option<ComponentTask>,
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
        .resource_overrides(ResourceOverrides::new().set("ButtonPadding", Thickness::uniform(6.0)))
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
    fn invalidate(&mut self) {
        self.revision += 1;
        self.busy = false;
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
        let revision = self.revision;
        let text = self.text.clone();
        let settings = self.settings.clone();
        let (source, target) = self.language_indices();
        let from = LANGUAGES[source].code.to_owned();
        let to = LANGUAGES[target].code.to_owned();
        self.task = Some(cx.spawn_background(move |cancel| {
            let entry = if from == "en" || from == "auto" {
                crate::dictionary::lookup(&text)
            } else {
                None
            };
            Message::Finished(
                revision,
                api::translate_cancellable(text, from, to, settings, &|| cancel.is_cancelled()),
                entry,
            )
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
    fn open_settings(&mut self, cx: &ComponentContext<Self>) {
        if self.settings_open && native::activate_settings() {
            return;
        }
        self.settings_open = cx.open_window(View::component::<Self>(WindowInput {
            settings_window: true,
            tab: "general".into(),
            parent: Some(cx.sender()),
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
        let height = if expanded && self.details_open {
            450.0
        } else if expanded {
            300.0
        } else if self.error {
            208.0
        } else {
            184.0
        };
        cx.window_title("翻译");
        cx.window_visuals(
            WindowVisuals::new()
                .client_size(620.0, 184.0)
                .backdrop(WindowBackdrop::Mica)
                .theme(theme(&self.settings)),
        );
        let dismiss = cx.message(Message::Dismiss);
        cx.use_effect(
            "popup-frame",
            (expanded, self.error, self.details_open),
            move || {
                native::configure_popup(620.0, height);
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
                icon(TRANSLATE)
                    .vertical_alignment(Center)
                    .margin(Thickness::new(4.0, 0.0, 10.0, 0.0)),
                ComboBox::new()
                    .items_source(LANGUAGES.iter().map(|l| l.label))
                    .selected_index(from)
                    .width(144.0)
                    .height(34.0)
                    .automation_name("源语言")
                    .on_selection_changed(cx.callback(Message::Source))
                    .grid_column(1),
                at(icon_button(SWAP, "交换语言", Message::Swap, cx), 0, 2),
                ComboBox::new()
                    .items_source(LANGUAGES.iter().skip(1).map(|l| l.label))
                    .selected_index(to - 1)
                    .width(132.0)
                    .height(34.0)
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
            .font_size(23.0)
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
            .height(60.0)
            .horizontal_alignment(Stretch)
            .automation_name("输入单词或文本")
            .on_text_changed(cx.callback(Message::Text));
        let result: View = if let Some(t) = &self.output {
            let is_word = t
                .source
                .trim()
                .chars()
                .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '\'');
            let pos = self
                .entry
                .as_ref()
                .map(|entry| entry.parts_of_speech.join(" · "))
                .filter(|s| !s.is_empty());
            let summary = StackPanel::new()
                .spacing(8.0)
                .margin(Thickness::new(0.0, 10.0, 0.0, 10.0))
                .children((
                    text(&t.target, 23.0).is_text_selection_enabled(self.entry.is_none()),
                    if is_word {
                        muted(pos.unwrap_or_else(|| "词性未收录".into())).into()
                    } else {
                        View::empty()
                    },
                ));
            if let Some(entry) = &self.entry {
                let senses = StackPanel::new().spacing(10.0).keyed_children(
                    entry.senses.iter().enumerate().map(|(i, sense)| {
                        (
                            i,
                            StackPanel::new().spacing(3.0).children((
                                muted(if sense.part_of_speech.is_empty() {
                                    "词性未收录"
                                } else {
                                    &sense.part_of_speech
                                }),
                                text(&sense.definition, 15.0).is_text_selection_enabled(true),
                            )),
                        )
                    }),
                );
                ScrollViewer::new()
                    .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                    .content(
                        Expander::new()
                            .horizontal_alignment(Stretch)
                            .is_expanded(self.details_open)
                            .automation_name("详细释义")
                            .on_is_expanded_changed(cx.callback(Message::Details))
                            .slots([
                                SlotView::new(ExpanderSlot::Header, summary),
                                SlotView::new(
                                    ExpanderSlot::Content,
                                    StackPanel::new()
                                        .spacing(12.0)
                                        .margin(Thickness::new(0.0, 12.0, 0.0, 12.0))
                                        .children((
                                            text(
                                                if entry.phonetic.is_empty() {
                                                    entry.word.clone()
                                                } else {
                                                    format!("{}  /{}/", entry.word, entry.phonetic)
                                                },
                                                14.0,
                                            ),
                                            senses,
                                            muted("ECDICT · 本地词典"),
                                        )),
                                ),
                            ]),
                    )
            } else {
                self.card(
                    ScrollViewer::new()
                        .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                        .content(summary),
                )
            }
        } else if self.error {
            text(&self.status, 12.0)
                .margin(Thickness::new(12.0, 2.0, 12.0, 0.0))
                .opacity(0.8)
                .into()
        } else {
            View::empty()
        };
        let engine = self
            .output
            .as_ref()
            .map(|t| {
                format!(
                    "{}{}",
                    t.provider.name(),
                    if t.note.contains("尚未配置") {
                        " · 未配置密钥"
                    } else if t.note.contains("网络") {
                        " · 网络不可用"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_else(|| self.settings.provider.name().into());
        let provider = self
            .output
            .as_ref()
            .map_or(self.settings.provider, |t| t.provider);
        let provider_data: &'static [u8] = match provider {
            Provider::Tencent => include_bytes!("../assets/providers/tencent.png"),
            Provider::Baidu => include_bytes!("../assets/providers/baidu.png"),
            Provider::Youdao => include_bytes!("../assets/providers/youdao.png"),
            Provider::DeepL => include_bytes!("../assets/providers/deepl.png"),
            Provider::Offline => include_bytes!("../assets/providers/local.png"),
            Provider::MyMemory => include_bytes!("../assets/providers/mymemory.png"),
        };
        let provider_icon: View = Image::new()
            .source_data(EncodedImage::from_static(provider_data))
            .width(18.0)
            .height(18.0)
            .stretch(windows_reactor::Stretch::Uniform)
            .into();
        let state_content = StackPanel::new()
            .orientation(Orientation::Horizontal)
            .spacing(8.0)
            .margin(Thickness::new(12.0, 0.0, 0.0, 0.0))
            .children((
                provider_icon,
                muted(if self.busy {
                    "翻译中…".into()
                } else {
                    engine
                })
                .vertical_alignment(Center),
            ));
        let state = Button::new()
            .style(ButtonStyle::Subtle)
            .horizontal_alignment(HorizontalAlignment::Left)
            .automation_name("切换翻译服务")
            .resource_overrides(
                ResourceOverrides::new().set("ButtonPadding", Thickness::new(0.0, 4.0, 8.0, 4.0)),
            )
            .on_click(cx.message(Message::CycleProvider))
            .content(state_content)
            .tooltip("切换翻译服务");
        let state = Grid::new().keyed_children([(self.settings.provider.index(), state)]);
        Border::new()
            .corner_radius(12.0)
            .padding(Thickness::new(14.0, 12.0, 14.0, 10.0))
            .on_pointer_pressed(cx.callback(|p: PointerEventInfo| {
                if p.window_y < 10.0 {
                    Message::Drag
                } else {
                    Message::Warmed
                }
            }))
            .content(
                Grid::new()
                    .rows([Auto, Auto, Star(1.0), Auto])
                    .row_spacing(7.0)
                    .children((
                        languages,
                        at(input, 1, 0),
                        at(result, 2, 0),
                        at(state, 3, 0),
                    )),
            )
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
        self.card(
            StackPanel::new().spacing(12.0).children((
                field(
                    "默认引擎",
                    ComboBox::new()
                        .items_source(Provider::ALL.iter().map(|p| p.name()))
                        .selected_index(provider.index())
                        .horizontal_alignment(Stretch)
                        .automation_name("默认引擎")
                        .on_selection_changed(cx.callback(Message::Provider)),
                ),
                credentials,
                extra,
                Button::new()
                    .style(ButtonStyle::Accent)
                    .on_click(cx.message(Message::Save))
                    .content("保存凭据"),
            )),
        )
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
        StackPanel::new().spacing(10.0).children((
            TextBox::new()
                .text(&self.query)
                .placeholder_text("搜索记录")
                .on_text_changed(cx.callback(Message::Search)),
            StackPanel::new().spacing(8.0).keyed_children(entries),
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
                            at(
                                ScrollViewer::new()
                                    .vertical_scroll_bar_visibility(ScrollBarVisibility::Auto)
                                    .content(content),
                                1,
                                0,
                            ),
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
        let (settings, history, mut warning) = storage::load();
        if !input.settings_window {
            let sender = cx.sender();
            native::install_handler(move |action| {
                let _ = sender.send(match action {
                    native::Action::Show => Message::Show,
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
            text: String::new(),
            output: None,
            entry: None,
            details_open: false,
            color_scheme: ColorScheme::Light,
            settings,
            history,
            query: String::new(),
            busy: false,
            revision: 0,
            task: None,
            status: warning.clone().unwrap_or_default(),
            error: warning.is_some(),
            input: ElementRef::new(),
        }
    }
    fn update(&mut self, message: Message, cx: &ComponentContext<Self>) {
        match message {
            Message::Text(value) => {
                if self.text != value {
                    self.text = value;
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
                self.invalidate();
                native::hide_popup();
            }
            Message::Show => {
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
            Message::Quit => {
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
            Message::CycleProvider => {
                let available: Vec<_> = Provider::ALL
                    .iter()
                    .copied()
                    .filter(|p| {
                        !p.needs_key()
                            || self.settings.credentials.get(p.index()).is_some_and(|c| {
                                !c.key.trim().is_empty()
                                    && (*p == Provider::DeepL || !c.id.trim().is_empty())
                            })
                    })
                    .collect();
                let current = self
                    .output
                    .as_ref()
                    .map_or(self.settings.provider, |t| t.provider);
                let next = available
                    .iter()
                    .position(|p| *p == current)
                    .map_or(0, |i| (i + 1) % available.len());
                self.settings.provider = available[next];
                self.save();
                self.schedule(cx);
            }
            Message::Provider(Some(i))
                if i < Provider::ALL.len() && self.settings.provider != Provider::ALL[i] =>
            {
                self.settings.provider = Provider::ALL[i];
                self.save();
                if !self.settings_window {
                    self.schedule(cx);
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
        if let Some(task) = self.task.take() {
            task.cancel();
        }
        if let Some(parent) = &self.parent {
            let _ = parent.send(Message::SettingsClosed);
        }
    }
}
