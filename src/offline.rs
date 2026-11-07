use anyhow::{Context, Result};
use ort::{environment::Environment, session::Session, value::Tensor};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokenizers::Tokenizer;

pub fn resource(relative: &str) -> PathBuf {
    let base = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_owned()))
        .unwrap_or_default();
    let packaged = base.join(relative);
    if packaged.exists() {
        packaged
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
    }
}
pub fn models_dir() -> PathBuf {
    std::env::var_os("LINGUA_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| resource("models"))
}
pub fn ready() -> bool {
    ["en-zh", "zh-en"].iter().all(|pair| {
        let root = models_dir().join(pair);
        [
            "encoder_model_quantized.onnx",
            "decoder_model_quantized.onnx",
            "source.spm",
            "vocab.json",
            "config.json",
        ]
        .iter()
        .all(|file| root.join(file).is_file())
    }) && resource("runtime/onnxruntime.dll").is_file()
}

struct Model {
    encoder: Session,
    decoder: Session,
    tokenizer: Tokenizer,
    vocab: Vec<String>,
    eos: i64,
    start: i64,
    target_prefix: Option<i64>,
    beams: usize,
}
pub struct OfflineEngine {
    environment: Option<Environment>,
    models: HashMap<String, Model>,
}
static ENGINE: OnceLock<Mutex<OfflineEngine>> = OnceLock::new();
fn engine() -> &'static Mutex<OfflineEngine> {
    ENGINE.get_or_init(|| {
        Mutex::new(OfflineEngine {
            environment: None,
            models: HashMap::new(),
        })
    })
}

fn english_word(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn check_budget(cancelled: &dyn Fn() -> bool, deadline: Instant) -> Result<()> {
    anyhow::ensure!(!cancelled(), "已取消");
    anyhow::ensure!(
        Instant::now() < deadline,
        "离线模型响应超时，请缩短文本或使用在线翻译"
    );
    Ok(())
}

fn repeated_ngram_tokens(sequence: &[i64], size: usize) -> Vec<i64> {
    if sequence.len() < size {
        return Vec::new();
    }
    let prefix = &sequence[sequence.len() - (size - 1)..];
    sequence
        .windows(size)
        .filter_map(|window| (window[..size - 1] == *prefix).then_some(window[size - 1]))
        .collect()
}

fn repetitive_output(text: &str, single_word: bool) -> bool {
    let characters: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let consecutive = if single_word { 3 } else { 5 };
    if characters
        .windows(consecutive)
        .any(|window| window.iter().all(|c| *c == window[0]))
    {
        return true;
    }
    (2..=8).any(|size| {
        characters.windows(size * 3).any(|window| {
            window[..size] == window[size..size * 2] && window[..size] == window[size * 2..]
        })
    })
}

impl OfflineEngine {
    fn model(&mut self, pair: &str) -> Result<&mut Model> {
        if !self.models.contains_key(pair) {
            let root = models_dir().join(pair);
            anyhow::ensure!(
                root.join("encoder_model_quantized.onnx").is_file(),
                "未找到 {pair} 离线模型，请运行 scripts/download-models.ps1"
            );
            if self.environment.is_none() {
                self.environment = Some(
                    ort::init_from(resource("runtime/onnxruntime.dll"))?
                        .with_name("TranslationTool")
                        .build()?,
                );
            }
            let env = self.environment.as_ref().unwrap();
            let threads = std::thread::available_parallelism()
                .map_or(2, |v| v.get())
                .clamp(1, 4);
            let encoder = Session::builder(env)?
                .with_intra_threads(threads)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .commit_from_file(root.join("encoder_model_quantized.onnx"))?;
            let decoder = Session::builder(env)?
                .with_intra_threads(threads)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .commit_from_file(root.join("decoder_model_quantized.onnx"))?;
            let tokenizer =
                crate::spm::tokenizer(root.join("source.spm"), root.join("vocab.json"))?;
            let words: HashMap<String, usize> =
                serde_json::from_slice(&std::fs::read(root.join("vocab.json"))?)?;
            // This model translates into several Chinese scripts. MarianTokenizer treats the
            // target-language code as one vocabulary id, outside SentencePiece segmentation.
            let target_prefix = if pair == "en-zh" {
                Some(
                    *words
                        .get(">>cmn_Hans<<")
                        .context("模型缺少简体中文语言标记")? as i64,
                )
            } else {
                None
            };
            let mut vocab = vec![String::new(); words.values().max().copied().unwrap_or(0) + 1];
            for (word, id) in words {
                vocab[id] = word;
            }
            let config: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.join("config.json"))?)?;
            self.models.insert(
                pair.to_owned(),
                Model {
                    encoder,
                    decoder,
                    tokenizer,
                    vocab,
                    eos: config["eos_token_id"].as_i64().unwrap_or(0),
                    start: config["decoder_start_token_id"].as_i64().unwrap_or(65000),
                    target_prefix,
                    beams: config["num_beams"].as_u64().unwrap_or(4).clamp(1, 8) as usize,
                },
            );
        }
        Ok(self.models.get_mut(pair).unwrap())
    }
}
impl Model {
    fn translate(
        &mut self,
        text: &str,
        cancelled: &dyn Fn() -> bool,
        deadline: Instant,
    ) -> Result<String> {
        check_budget(cancelled, deadline)?;
        let single_word = self.target_prefix.is_some() && english_word(text);
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let source_tokens = encoding.get_ids().len();
        // A word must not turn into a paragraph. Sentence budgets scale with the
        // source and remain bounded because this decoder has no KV cache.
        let max_new_tokens = if single_word {
            12
        } else {
            (source_tokens * 3 + 12).clamp(16, 96)
        };
        let beam_count = if single_word { 1 } else { self.beams };
        let mut input: Vec<i64> = self
            .target_prefix
            .into_iter()
            .chain(encoding.get_ids().iter().map(|x| i64::from(*x)))
            .collect();
        if input.last() != Some(&self.eos) {
            input.push(self.eos);
        }
        anyhow::ensure!(
            input.len() <= 256,
            "这一段过长，请拆成短句后翻译（每段最多 256 个 token）"
        );
        let length = input.len();
        let encoded = self.encoder.run(ort::inputs![
            "input_ids" => Tensor::from_array(([1, length], input))?,
            "attention_mask" => Tensor::from_array(([1, length], vec![1_i64; length]))?,
        ])?;
        check_budget(cancelled, deadline)?;
        let (hidden_shape, hidden) = encoded["last_hidden_state"].try_extract_tensor::<f32>()?;
        let hidden_shape = hidden_shape.to_vec();
        let hidden = hidden.to_vec();
        let mut beams = vec![(vec![self.start], 0.0_f64)];
        let mut finished: Vec<(Vec<i64>, f64)> = Vec::new();
        let mut complete = false;
        for _ in 0..max_new_tokens {
            check_budget(cancelled, deadline)?;
            let batch = beams.len();
            let tokens = beams[0].0.len();
            let mut shape = hidden_shape.clone();
            shape[0] = batch as i64;
            let result = self.decoder.run(ort::inputs![
                "input_ids" => Tensor::from_array(([batch, tokens], beams.iter().flat_map(|b| b.0.iter().copied()).collect::<Vec<_>>()))?,
                "encoder_hidden_states" => Tensor::from_array((shape, hidden.repeat(batch)))?,
                "encoder_attention_mask" => Tensor::from_array(([batch, length], vec![1_i64; batch * length]))?,
            ])?;
            // A run is synchronous; reject it immediately if this decoder step
            // crossed the request deadline, before considering any output.
            check_budget(cancelled, deadline)?;
            let (shape, logits) = result["logits"].try_extract_tensor::<f32>()?;
            let vocab_size = *shape.last().context("无效模型输出")? as usize;
            let mut candidates = Vec::with_capacity(batch * beam_count * 2);
            for (beam, (sequence, score)) in beams.iter().enumerate() {
                let offset = (beam * tokens + tokens - 1) * vocab_size;
                let last = &logits[offset..offset + vocab_size];
                let banned = repeated_ngram_tokens(sequence, if single_word { 2 } else { 3 });
                let mut best: Vec<(usize, f32)> = last
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(id, value)| {
                        *id != self.start as usize
                            && value.is_finite()
                            && !banned.contains(&(*id as i64))
                    })
                    .collect();
                let maximum = best
                    .iter()
                    .map(|(_, value)| *value)
                    .fold(f32::NEG_INFINITY, f32::max);
                anyhow::ensure!(maximum.is_finite(), "离线模型输出异常，请改用在线翻译");
                let log_sum = best
                    .iter()
                    .map(|(_, value)| f64::from(*value - maximum).exp())
                    .sum::<f64>()
                    .ln()
                    + f64::from(maximum);
                let count = (beam_count * 2).min(best.len());
                best.select_nth_unstable_by(count - 1, |a, b| b.1.total_cmp(&a.1));
                candidates.extend(
                    best[..count].iter().map(|(id, logit)| {
                        (beam, *id as i64, *score + f64::from(*logit) - log_sum)
                    }),
                );
            }
            candidates.sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
            let mut next = Vec::with_capacity(beam_count);
            for (rank, (beam, token, score)) in candidates.into_iter().enumerate() {
                if token == self.eos {
                    if rank < beam_count {
                        // A smaller length penalty avoids promoting long low-
                        // information continuations over an already complete answer.
                        finished.push((beams[beam].0.clone(), score / (tokens as f64).powf(0.7)));
                    }
                } else if next.len() < beam_count {
                    let mut sequence = beams[beam].0.clone();
                    sequence.push(token);
                    next.push((sequence, score));
                }
                if next.len() == beam_count {
                    break;
                }
            }
            finished.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
            finished.truncate(beam_count);
            if next.is_empty() {
                complete = !finished.is_empty();
                break;
            }
            // Stop once the requested number of complete hypotheses exists;
            // never keep searching for an increasingly long normalized winner.
            if finished.len() == beam_count {
                complete = true;
                break;
            }
            beams = next;
        }
        check_budget(cancelled, deadline)?;
        anyhow::ensure!(complete, "离线模型未能在合理长度内完成翻译，请改用在线翻译");
        let (output, _) = finished
            .into_iter()
            .next()
            .context("译文超过生成长度上限，请拆成较短的句子后重试")?;
        let words: String = output
            .into_iter()
            .skip(1)
            .filter_map(|id| self.vocab.get(id as usize))
            .map(String::as_str)
            .collect();
        let mut result = words.replace('▁', " ").trim().to_owned();
        for (from, to) in [
            (" .", "."),
            (" ,", ","),
            (" !", "!"),
            (" ?", "?"),
            (" n't", "n't"),
            (" 'm", "'m"),
            (" 's", "'s"),
            (" 've", "'ve"),
            (" 're", "'re"),
            (" 'd", "'d"),
            (" 'll", "'ll"),
        ] {
            result = result.replace(from, to);
        }
        anyhow::ensure!(!result.is_empty(), "离线模型没有生成译文");
        let character_limit = if single_word {
            24
        } else {
            (text.chars().count() * 6 + 24).clamp(64, 1536)
        };
        anyhow::ensure!(
            result.chars().count() <= character_limit
                && !repetitive_output(&result, single_word)
                && !result.contains("<unk>")
                && !result.contains(">>"),
            "离线模型生成了异常译文，已拒绝显示，请改用在线翻译"
        );
        Ok(result)
    }
}
#[cfg(test)]
pub fn translate(text: &str, from: &str, to: &str) -> Result<String> {
    translate_cancellable(text, from, to, &|| false)
}
pub fn translate_cancellable(
    text: &str,
    from: &str,
    to: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<String> {
    anyhow::ensure!(!cancelled(), "已取消");
    anyhow::ensure!(
        matches!((from, to), ("en", "zh") | ("zh", "en")),
        "离线模式目前支持中文与英语互译，请选择中英语言对"
    );
    let text = text.trim();
    let single_word = from == "en" && to == "zh" && english_word(text);
    if single_word {
        if let Some(entry) = crate::dictionary::lookup(text) {
            if let Some(definition) = entry
                .senses
                .iter()
                .map(|sense| sense.definition.trim())
                .find(|definition| !definition.is_empty())
            {
                anyhow::ensure!(!cancelled(), "已取消");
                return Ok(definition.to_owned());
            }
        }
    }
    let deadline = Instant::now() + Duration::from_millis(if single_word { 1500 } else { 3000 });
    let mut engine = loop {
        check_budget(cancelled, deadline)?;
        match engine().try_lock() {
            Ok(engine) => break engine,
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(_) => anyhow::bail!("离线引擎不可用"),
        }
    };
    let model = engine.model(&format!("{from}-{to}"))?;
    check_budget(cancelled, deadline)?;
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                Ok(String::new())
            } else {
                model.translate(line.trim(), cancelled, deadline)
            }
        })
        .collect::<Result<Vec<_>>>()
        .map(|v| v.join("\n"))
}
pub fn warm_up() {
    let _ = engine().lock().map(|mut e| {
        let _ = e.model("en-zh");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dictionary_fast_path_only_accepts_a_single_english_word() {
        for word in ["snake", "Sad", "BANK"] {
            assert!(english_word(word));
        }
        for text in [
            "",
            "the bank",
            "snake\nbank",
            "bank-account",
            "can't",
            "你好",
            "snake2",
        ] {
            assert!(!english_word(text));
        }
    }
    #[test]
    fn abnormal_repetition_is_rejected_without_deduplicating_text() {
        assert!(repetitive_output("伤伤伤", true));
        assert!(repetitive_output("银行银行银行", false));
        assert!(!repetitive_output("我知道你知道这件事。", false));
        assert!(!repetitive_output("The bank is closed today.", false));
    }
    #[test]
    #[ignore = "requires the downloaded dictionary, ONNX runtime and models"]
    fn real_words_and_sentence_finish_within_a_bounded_time() {
        for word in ["snake", "sad", "bank"] {
            let start = Instant::now();
            let result = translate(word, "en", "zh").unwrap();
            println!(
                "dictionary {word} => {result} ({} ms)",
                start.elapsed().as_millis()
            );
            assert!(start.elapsed() < Duration::from_secs(2));
            assert!(result.chars().count() <= 120);
            assert!(
                result
                    .chars()
                    .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
            );
        }
        let start = Instant::now();
        let result = translate("The bank is closed today.", "en", "zh").unwrap();
        println!("sentence => {result} ({} ms)", start.elapsed().as_millis());
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(!repetitive_output(&result, false));
        assert!(result.chars().count() <= 120);

        // Bypass the dictionary so the actual model, including the former snake
        // regression, must either finish concisely or reject the generation.
        let mut engine = engine().lock().unwrap();
        let model = engine.model("en-zh").unwrap();
        for word in ["snake", "sad", "bank"] {
            let start = Instant::now();
            let result = model.translate(word, &|| false, start + Duration::from_millis(1500));
            println!(
                "bounded model {word}: {result:?} ({} ms)",
                start.elapsed().as_millis()
            );
            assert!(start.elapsed() < Duration::from_secs(3));
            if let Ok(result) = result {
                assert!(result.chars().count() <= 24);
                assert!(!repetitive_output(&result, true));
            }
        }
    }
    #[test]
    #[ignore = "requires the downloaded local ONNX runtime and models"]
    fn real_offline_translation_both_directions() {
        assert!(ready(), "offline assets missing");
        for (text, from, to) in [
            ("Hello world!", "en", "zh"),
            ("今天天气很好。", "zh", "en"),
            ("Good design makes life simpler.", "en", "zh"),
            ("sad", "en", "zh"),
            ("happy", "en", "zh"),
            ("I feel sad today.", "en", "zh"),
            ("The bank is closed today.", "en", "zh"),
            ("The cat is sleeping on the sofa.", "en", "zh"),
            ("我今天有点难过。", "zh", "en"),
            ("请在明天上午九点提醒我。", "zh", "en"),
        ] {
            let start = std::time::Instant::now();
            let result = translate(text, from, to).unwrap();
            println!(
                "{from}->{to}: {text} => {result} ({} ms)",
                start.elapsed().as_millis()
            );
            assert!(!result.is_empty());
            assert_ne!(result, text);
            if to == "zh" {
                assert!(
                    result
                        .chars()
                        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
                );
            } else {
                assert!(result.chars().any(|c| c.is_ascii_alphabetic()));
            }
            if text == "sad" {
                assert!(
                    ["悲伤", "难过", "伤心", "哀伤", "悲哀", "忧愁"]
                        .iter()
                        .any(|word| result.contains(word))
                );
            }
            if text == "happy" {
                assert!(
                    ["快乐", "高兴", "开心", "幸福"]
                        .iter()
                        .any(|word| result.contains(word))
                );
            }
        }
    }
    #[test]
    fn cancelled_inference_returns_before_loading_assets() {
        let start = std::time::Instant::now();
        assert!(
            translate_cancellable("input", "en", "zh", &|| true)
                .unwrap_err()
                .to_string()
                .contains("取消")
        );
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
    }
}
