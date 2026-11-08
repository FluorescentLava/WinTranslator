use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    Tencent,
    Baidu,
    Youdao,
    DeepL,
    Offline,
    #[default]
    MyMemory,
}
impl Provider {
    pub const ALL: [Self; 6] = [
        Self::Tencent,
        Self::Baidu,
        Self::Youdao,
        Self::DeepL,
        Self::Offline,
        Self::MyMemory,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Tencent => "腾讯翻译",
            Self::Baidu => "百度翻译",
            Self::Youdao => "有道翻译",
            Self::DeepL => "DeepL",
            Self::Offline => "本地离线",
            Self::MyMemory => "MyMemory",
        }
    }
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|x| *x == self).unwrap_or(0)
    }
    pub fn needs_key(self) -> bool {
        !matches!(self, Self::Offline | Self::MyMemory)
    }
}

#[derive(Clone, Copy)]
pub struct Language {
    pub label: &'static str,
    pub code: &'static str,
}
pub const LANGUAGES: [Language; 12] = [
    Language {
        label: "自动检测",
        code: "auto",
    },
    Language {
        label: "中文(简体)",
        code: "zh",
    },
    Language {
        label: "英语",
        code: "en",
    },
    Language {
        label: "日语",
        code: "ja",
    },
    Language {
        label: "韩语",
        code: "ko",
    },
    Language {
        label: "法语",
        code: "fr",
    },
    Language {
        label: "德语",
        code: "de",
    },
    Language {
        label: "西班牙语",
        code: "es",
    },
    Language {
        label: "俄语",
        code: "ru",
    },
    Language {
        label: "葡萄牙语",
        code: "pt",
    },
    Language {
        label: "意大利语",
        code: "it",
    },
    Language {
        label: "阿拉伯语",
        code: "ar",
    },
];
pub fn detect(text: &str) -> &'static str {
    if text.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)) {
        "ja"
    } else if text.chars().any(|c| ('\u{ac00}'..='\u{d7af}').contains(&c)) {
        "ko"
    } else if text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
        "zh"
    } else if text.chars().any(|c| ('\u{0400}'..='\u{04ff}').contains(&c)) {
        "ru"
    } else if text.chars().any(|c| ('\u{0600}'..='\u{06ff}').contains(&c)) {
        "ar"
    } else {
        "en"
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Credential {
    pub id: String,
    pub key: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: Provider,
    pub theme: usize,
    pub auto_offline: bool,
    pub region: String,
    pub deepl_free: bool,
    pub from_index: usize,
    pub to_index: usize,
    pub auto_pair: bool,
    pub auto_start: bool,
    #[serde(skip)]
    pub credentials: [Credential; 4],
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: Provider::MyMemory,
            theme: 0,
            auto_offline: true,
            region: "ap-guangzhou".into(),
            deepl_free: true,
            from_index: 1,
            to_index: 2,
            auto_pair: true,
            auto_start: true,
            credentials: Default::default(),
        }
    }
}
impl Settings {
    pub fn follow_bilingual_input(&mut self, text: &str) {
        if text.trim().is_empty()
            || !matches!(self.from_index, 0..=2)
            || !matches!(self.to_index, 1 | 2)
        {
            return;
        }
        let direction = match detect(text) {
            "zh" => (1, 2),
            "en" => (2, 1),
            _ => return,
        };
        self.from_index = direction.0;
        self.to_index = direction.1;
        self.auto_pair = true;
    }

    pub fn credential(&self) -> Credential {
        self.credentials
            .get(self.provider.index())
            .cloned()
            .unwrap_or_default()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Translation {
    pub source: String,
    pub target: String,
    pub from: String,
    pub to: String,
    pub provider: Provider,
    pub elapsed_ms: u64,
    pub created: String,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub note: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripts_and_japanese_priority() {
        assert_eq!(detect("Hello world"), "en");
        assert_eq!(detect("你好世界"), "zh");
        assert_eq!(detect("今日はいい天気"), "ja");
        assert_eq!(detect("안녕하세요"), "ko");
    }

    #[test]
    fn editing_bilingual_input_follows_its_language_after_a_manual_swap() {
        let mut settings = Settings {
            from_index: 2,
            to_index: 1,
            auto_pair: false,
            ..Settings::default()
        };
        settings.follow_bilingual_input("这是一段需要翻译的中文。");
        assert_eq!((settings.from_index, settings.to_index), (1, 2));
        assert!(settings.auto_pair);
        settings.follow_bilingual_input("Now translate this English paragraph.");
        assert_eq!((settings.from_index, settings.to_index), (2, 1));

        settings.from_index = 3;
        settings.to_index = 1;
        settings.auto_pair = false;
        settings.follow_bilingual_input("中文引用");
        assert_eq!((settings.from_index, settings.to_index), (3, 1));
        assert!(!settings.auto_pair);
    }
}
