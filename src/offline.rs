use anyhow::{Context, Result};
use ort::{
    environment::Environment,
    session::{RunOptions, Session},
    value::Tensor,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
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
pub fn download_dir() -> PathBuf {
    std::env::var_os("LINGUA_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::storage::data_dir().join("models"))
}

fn select_models_dir(downloaded: PathBuf, packaged: Option<PathBuf>) -> PathBuf {
    if crate::model_download::ready_at(&downloaded) {
        return downloaded;
    }
    packaged
        .filter(|path| crate::model_download::ready_at(path))
        .unwrap_or(downloaded)
}

pub fn models_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("LINGUA_MODELS_DIR") {
        return PathBuf::from(path);
    }
    let packaged = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("models")));
    select_models_dir(download_dir(), packaged)
}

pub fn uninstall_models() -> Result<()> {
    let _engine = unload_models_for_removal()?;
    crate::model_download::remove_at(&download_dir())?;
    if std::env::var_os("LINGUA_MODELS_DIR").is_none()
        && let Some(packaged) = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("models")))
    {
        crate::model_download::remove_at(&packaged)?;
    }
    Ok(())
}

/// A cancelled download owns only the configured download directory; it must
/// never remove an already packaged model or another model's resources.
pub(crate) fn cancel_model_download(root: &Path) -> Result<()> {
    let _engine = unload_models_for_removal()?;
    crate::model_download::remove_download_at(root)
}

fn unload_models_for_removal() -> Result<MutexGuard<'static, OfflineEngine>> {
    // The caller sets the maintenance flag before spawning this worker. Stop a
    // currently executing ONNX graph before waiting for its engine lock, then
    // drop every mapped session before attempting any Windows file deletion.
    interrupt_active_inference()?;
    let mut engine = engine()
        .lock()
        .map_err(|_| anyhow::anyhow!("离线引擎不可用，请重启程序后移除模型"))?;
    engine.models.clear();
    Ok(engine)
}

pub fn ready() -> bool {
    !crate::model_download::is_removing()
        && crate::model_download::ready_at(&models_dir())
        && resource("runtime/onnxruntime.dll").is_file()
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
    length_penalty: f64,
    early_stopping: bool,
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

fn active_inference() -> &'static Mutex<Option<Arc<RunOptions>>> {
    static ACTIVE: OnceLock<Mutex<Option<Arc<RunOptions>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

struct ActiveInference(Arc<RunOptions>);

impl ActiveInference {
    fn new() -> Result<Self> {
        let options = Arc::new(RunOptions::new()?);
        let mut active = active_inference()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Registration and the removal worker's snapshot share this mutex. If
        // maintenance starts between checkpoints, it either sees this options
        // object or registration refuses to launch a new graph.
        anyhow::ensure!(!crate::model_download::is_removing(), "已取消");
        *active = Some(Arc::clone(&options));
        Ok(Self(options))
    }
}

impl Drop for ActiveInference {
    fn drop(&mut self) {
        let mut active = active_inference()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|options| Arc::ptr_eq(options, &self.0))
        {
            *active = None;
        }
    }
}

fn interrupt_active_inference() -> Result<()> {
    let options = active_inference()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(options) = options {
        options
            .terminate()
            .map_err(|_| anyhow::anyhow!("无法停止离线推理，请稍后重试移除模型"))?;
    }
    Ok(())
}

fn english_word(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn check_budget(cancelled: &dyn Fn() -> bool, deadline: Instant) -> Result<()> {
    anyhow::ensure!(!cancelled(), "已取消");
    anyhow::ensure!(
        Instant::now() < deadline,
        "离线模型本段处理超时，请重试或使用在线翻译"
    );
    Ok(())
}

// Tokenization and decoder tensors are bounded independently of the size of a
// pasted document. In particular, do not tokenize a whole unbroken paragraph
// merely to find out that it needs to be split.
const CHUNK_SOURCE_TOKENS: usize = 48;
const CHUNK_SCAN_CHARACTERS: usize = 384;

fn source_encoding(tokenizer: &Tokenizer, text: &str) -> Result<tokenizers::Encoding> {
    tokenizer
        .encode(text, false)
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn next_source_chunk<'a>(tokenizer: &Tokenizer, remaining: &'a str) -> Result<&'a str> {
    anyhow::ensure!(!remaining.is_empty(), "待翻译分段为空");
    let window_end = remaining
        .char_indices()
        .nth(CHUNK_SCAN_CHARACTERS)
        .map_or(remaining.len(), |(offset, _)| offset);
    let window = &remaining[..window_end];
    let encoding = source_encoding(tokenizer, window)?;
    let mut end = if encoding.len() <= CHUNK_SOURCE_TOKENS {
        window.len()
    } else {
        encoding
            .get_offsets()
            .iter()
            .take(CHUNK_SOURCE_TOKENS)
            .map(|(_, end)| *end)
            .max()
            .unwrap_or(0)
            .min(window.len())
    };
    while end > 0 && !window.is_char_boundary(end) {
        end -= 1;
    }
    // Normalization can map several pieces to one original character, and a
    // word can tokenize differently when it ends at a chunk boundary. Verify
    // the actual prefix rather than trusting offsets or dropping extra tokens.
    while end > 0 && source_encoding(tokenizer, &window[..end])?.len() > CHUNK_SOURCE_TOKENS {
        end = window[..end]
            .char_indices()
            .next_back()
            .map_or(0, |(offset, _)| offset);
    }
    anyhow::ensure!(end > 0, "离线模型无法对这一字符分段");
    if end < remaining.len() {
        let minimum_boundary = end / 3;
        let mut sentence_end = None;
        let mut word_end = None;
        for (offset, character) in window[..end].char_indices() {
            let boundary = offset + character.len_utf8();
            if boundary < minimum_boundary {
                continue;
            }
            if character.is_whitespace() {
                word_end = Some(boundary);
            }
            let following = remaining[boundary..].chars().next();
            if matches!(character, '。' | '！' | '？' | '；')
                || (matches!(character, '.' | '!' | '?' | ';')
                    && following.is_none_or(|next| {
                        next.is_whitespace() || matches!(next, '\"' | '\'' | '”' | '’')
                    }))
            {
                sentence_end = Some(boundary);
            }
        }
        if let Some(boundary) = sentence_end.or(word_end) {
            end = boundary;
        }
        // The preferred sentence/word boundary may also change tokenization.
        while source_encoding(tokenizer, &window[..end])?.len() > CHUNK_SOURCE_TOKENS {
            end = window[..end]
                .char_indices()
                .next_back()
                .map_or(0, |(offset, _)| offset);
            anyhow::ensure!(end > 0, "离线模型无法对这一字符分段");
        }
    }
    Ok(&remaining[..end])
}

fn append_chunk_translation(output: &mut String, translated: &str, to: &str) {
    let previous = output.chars().next_back();
    let next = translated.chars().next();
    if previous.is_some_and(|character| !character.is_whitespace())
        && (to == "en"
            || (previous.is_some_and(|character| character.is_ascii_alphanumeric())
                && next.is_some_and(|character| character.is_ascii_alphanumeric())))
    {
        output.push(' ');
    }
    output.push_str(translated);
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
        anyhow::ensure!(!crate::model_download::is_removing(), "正在卸载离线模型");
        if !self.models.contains_key(pair) {
            let root = models_dir().join(pair);
            anyhow::ensure!(
                crate::model_download::ready_at(&models_dir()),
                "离线模型尚未下载完成，请在设置 → 翻译服务中下载离线模型"
            );
            anyhow::ensure!(
                resource("runtime/onnxruntime.dll").is_file(),
                "离线推理库缺失，请重新运行安装程序"
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
            anyhow::ensure!(!crate::model_download::is_removing(), "正在移除离线模型");
            let decoder = Session::builder(env)?
                .with_intra_threads(threads)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .commit_from_file(root.join("decoder_model_quantized.onnx"))?;
            anyhow::ensure!(!crate::model_download::is_removing(), "正在移除离线模型");
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
                    length_penalty: config["length_penalty"]
                        .as_f64()
                        .filter(|value| value.is_finite() && *value > 0.0)
                        .unwrap_or(1.0),
                    early_stopping: config["early_stopping"].as_bool().unwrap_or(false),
                },
            );
        }
        Ok(self.models.get_mut(pair).unwrap())
    }
}
impl Model {
    #[cfg(test)]
    fn translate(
        &mut self,
        text: &str,
        cancelled: &dyn Fn() -> bool,
        deadline: Instant,
    ) -> Result<String> {
        self.translate_chunk(text, cancelled, deadline, None)
    }

    fn translate_chunk(
        &mut self,
        text: &str,
        cancelled: &dyn Fn() -> bool,
        deadline: Instant,
        beam_limit: Option<usize>,
    ) -> Result<String> {
        check_budget(cancelled, deadline)?;
        let inference = ActiveInference::new()?;
        let single_word =
            beam_limit.is_none() && self.target_prefix.is_some() && english_word(text);
        let encoding = source_encoding(&self.tokenizer, text)?;
        let source_tokens = encoding.get_ids().len();
        // A word must not turn into a paragraph. Sentence budgets scale with the
        // source and remain bounded because this decoder has no KV cache.
        let max_new_tokens = if single_word {
            12
        } else if beam_limit.is_some() {
            (source_tokens * 3 + 16).clamp(24, 160)
        } else {
            (source_tokens * 3 + 12).clamp(16, 96)
        };
        let beam_count = if single_word {
            1
        } else {
            beam_limit.map_or(self.beams, |limit| self.beams.min(limit.max(1)))
        };
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
        let encoded = self.encoder.run_with_options(
            ort::inputs![
                "input_ids" => Tensor::from_array(([1, length], input))?,
                "attention_mask" => Tensor::from_array(([1, length], vec![1_i64; length]))?,
            ],
            &*inference.0,
        );
        check_budget(cancelled, deadline)?;
        let encoded = encoded?;
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
            let result = self.decoder.run_with_options(
                ort::inputs![
                    "input_ids" => Tensor::from_array(([batch, tokens], beams.iter().flat_map(|b| b.0.iter().copied()).collect::<Vec<_>>()))?,
                    "encoder_hidden_states" => Tensor::from_array((shape, hidden.repeat(batch)))?,
                    "encoder_attention_mask" => Tensor::from_array(([batch, length], vec![1_i64; batch * length]))?,
                ],
                &*inference.0,
            );
            // A run is synchronous; reject it immediately if this decoder step
            // crossed the request deadline, before considering any output.
            check_budget(cancelled, deadline)?;
            let result = result?;
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
            let best_candidate_score = candidates
                .first()
                .map_or(f64::NEG_INFINITY, |candidate| candidate.2);
            let mut next = Vec::with_capacity(beam_count);
            for (rank, (beam, token, score)) in candidates.into_iter().enumerate() {
                if token == self.eos {
                    if rank < beam_count {
                        finished.push((
                            beams[beam].0.clone(),
                            score / (tokens as f64).powf(self.length_penalty),
                        ));
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
            // Match the normal Marian beam-search heuristic instead of stopping
            // immediately after collecting a set of short answers. The deadline
            // and token cap remain in force even while the search continues.
            let best_live_score = best_candidate_score / (tokens as f64).powf(self.length_penalty);
            if finished.len() == beam_count
                && (self.early_stopping
                    || finished
                        .last()
                        .is_some_and(|(_, score)| *score >= best_live_score))
            {
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
    let single_word = from == "en" && to == "zh" && text.len() <= 64 && english_word(text);
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
    let external_cancelled = cancelled;
    let cancelled = &|| external_cancelled() || crate::model_download::is_removing();
    let lock_deadline = Instant::now() + Duration::from_secs(30);
    let mut engine = loop {
        anyhow::ensure!(!cancelled(), "已取消");
        anyhow::ensure!(Instant::now() < lock_deadline, "离线引擎正忙，请稍后再试");
        match engine().try_lock() {
            Ok(engine) => break engine,
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(_) => anyhow::bail!("离线引擎不可用"),
        }
    };
    // Each chunk has an independent inference budget that excludes model loading.
    let model = engine.model(&format!("{from}-{to}"))?;
    anyhow::ensure!(!cancelled(), "已取消");
    let long_document = text.chars().take(241).count() > 240;
    let mut result = String::new();
    let mut chunk_index = 0;
    for line in text.split_inclusive('\n') {
        anyhow::ensure!(!cancelled(), "已取消");
        let (body, newline) = if let Some(body) = line.strip_suffix("\r\n") {
            (body, "\r\n")
        } else if let Some(body) = line.strip_suffix('\n') {
            (body, "\n")
        } else {
            (line, "")
        };
        let content = body.trim();
        if content.is_empty() {
            result.push_str(body);
            result.push_str(newline);
            continue;
        }
        let leading = body.len() - body.trim_start().len();
        let trailing = body.trim_end().len();
        result.push_str(&body[..leading]);
        let mut remaining = content;
        let mut paragraph = String::new();
        let mut reduced_beams = long_document;
        while !remaining.is_empty() {
            anyhow::ensure!(!cancelled(), "已取消");
            let chunk = next_source_chunk(&model.tokenizer, remaining)?;
            reduced_beams |= chunk.len() < remaining.len();
            remaining = &remaining[chunk.len()..];
            if chunk.trim().is_empty() {
                paragraph.push_str(chunk);
                continue;
            }
            chunk_index += 1;
            let duration = if reduced_beams {
                Duration::from_secs(8)
            } else if single_word {
                Duration::from_millis(1500)
            } else {
                Duration::from_secs(3)
            };
            let translated = model
                .translate_chunk(
                    chunk.trim(),
                    cancelled,
                    Instant::now() + duration,
                    reduced_beams.then_some(1),
                )
                .map_err(|error| anyhow::anyhow!("离线翻译第 {chunk_index} 段未完成：{error}"))?;
            append_chunk_translation(&mut paragraph, &translated, to);
        }
        result.push_str(&paragraph);
        result.push_str(&body[trailing..]);
        result.push_str(newline);
    }
    anyhow::ensure!(!cancelled(), "已取消");
    Ok(result)
}
pub fn warm_up() {
    let _ = engine().lock().map(|mut e| {
        let _ = e.model("en-zh");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character_tokenizer() -> Tokenizer {
        let mut pieces = vec![("<unk>".to_owned(), 0.0)];
        let mut characters: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,!?;\r\n\t你好世界这是长段文字没有空格第二行最后结束。！？；🙂é"
            .chars()
            .collect();
        characters.sort_unstable();
        characters.dedup();
        pieces.extend(
            characters
                .into_iter()
                .map(|character| (character.to_string(), 0.0)),
        );
        Tokenizer::new(tokenizers::models::unigram::Unigram::from(pieces, Some(0), false).unwrap())
    }

    #[test]
    fn chunks_preserve_all_source_characters_with_bounded_tokens() {
        let tokenizer = character_tokenizer();
        assert_eq!(
            next_source_chunk(
                &tokenizer,
                "The library is open. Another sentence follows after this complete sentence."
            )
            .unwrap(),
            "The library is open."
        );
        for source in [
            "A sentence ends here. Another sentence follows with several words. ".repeat(120),
            "abcdefghijklmnopqrstuvwxyz".repeat(220),
            "你好世界这是长段文字没有空格。第二行最后结束！？🙂é".repeat(240),
            format!(
                "  First paragraph.\r\n\r\n\tSecond line!\n{}\n\nLast line.  ",
                " ".repeat(900)
            ),
        ] {
            let mut remaining = source.as_str();
            let mut restored = String::new();
            while !remaining.is_empty() {
                let chunk = next_source_chunk(&tokenizer, remaining).unwrap();
                assert!(!chunk.is_empty());
                assert!(chunk.chars().count() <= CHUNK_SCAN_CHARACTERS);
                assert!(source_encoding(&tokenizer, chunk).unwrap().len() <= CHUNK_SOURCE_TOKENS);
                restored.push_str(chunk);
                remaining = &remaining[chunk.len()..];
            }
            assert_eq!(restored, source);
        }
    }

    #[test]
    fn translated_chunks_keep_english_words_and_sentences_separate() {
        let mut english = String::new();
        append_chunk_translation(&mut english, "The library is open.", "en");
        append_chunk_translation(&mut english, "Please come tomorrow.", "en");
        assert_eq!(english, "The library is open. Please come tomorrow.");
        let mut chinese = String::new();
        append_chunk_translation(&mut chinese, "图书馆开门了。", "zh");
        append_chunk_translation(&mut chinese, "请明天再来。", "zh");
        assert_eq!(chinese, "图书馆开门了。请明天再来。");
        append_chunk_translation(&mut chinese, "Alpha", "zh");
        append_chunk_translation(&mut chinese, "Beta", "zh");
        assert!(chinese.ends_with("Alpha Beta"));
    }

    fn long_english_passage() -> String {
        [
            "The town library opens at nine every morning. On Monday, a group of volunteers arrived early to prepare a new reading room. They moved the small tables closer to the windows and placed the children's books on lower shelves. A teacher brought a map of the region, while a retired engineer repaired the clock above the entrance. By lunchtime, every chair was ready and the first visitors were waiting outside. The librarian thanked everyone and explained how to borrow a book without paying a fee.",
            "Across the river, the market was beginning another busy day. Farmers arranged fresh vegetables in wooden boxes, and a baker carried warm bread from a nearby shop. A student compared prices before choosing apples for her family. The manager asked delivery drivers to leave a clear path for people using wheelchairs. When a brief shower started, the sellers opened their umbrellas and helped one another cover the fruit. The rain stopped quickly, and the square soon filled with conversation again.",
            "At the local school, the science class was studying the movement of water. The students poured equal amounts into different containers and recorded the height of each surface. Their teacher asked them to explain why a wide bowl looked shallower than a narrow glass. One group drew a careful diagram, while another checked the measurements a second time. After the experiment, the class cleaned the tables and wrote down the questions they wanted to investigate during the following lesson.",
            "The railway station has recently installed clearer signs near its platforms. Visitors can now find the ticket office, elevators, and bus connections without asking for directions. The new timetable uses larger letters and shows both the departure time and the final destination. Station staff tested the signs with passengers of different ages before approving the design. They also placed a quiet waiting area beside the information desk, where families can sit safely until their train is announced.",
            "A small museum on the hill tells the story of the town's oldest bridge. Photographs show workers carrying stone across the valley before modern machinery was available. An interactive display explains how the arches support the road above them. The guide encouraged children to build their own bridge from wooden blocks and compare the results. Several visitors stayed after the tour to ask about the restoration project. The museum plans to publish the original construction drawings later this year.",
            "In the community garden, neighbors grow flowers, herbs, and seasonal vegetables. Each household looks after a small plot, but the tools and water tanks are shared. On Saturday, an experienced gardener showed new members how to protect young plants from strong wind. Another volunteer explained why fallen leaves can improve the soil instead of going into household rubbish. The group agreed to keep a written record of planting dates so that next year's gardeners can learn from the same observations.",
            "The health center is organizing an evening workshop about sleep and daily routines. A nurse will describe how regular meals, daylight, and gentle exercise can help people feel more rested. Participants are invited to write down a question before the meeting begins. The organizers have reserved seats for caregivers who cannot attend during working hours. Printed information will be available in several languages, and a volunteer will help visitors find reliable services if they need further advice.",
            "Meanwhile, a team of engineers is inspecting the public swimming pool. They are checking the filters, emergency lights, and handrails before the summer season starts. The supervisor asked each worker to record the condition of the equipment rather than relying on memory. Any damaged part will be replaced before the doors reopen. The final inspection will include a practice evacuation with the staff, followed by a review of the instructions posted in the changing rooms.",
            "The youth orchestra rehearses in a room behind the theater. This week, the musicians are learning a piece that begins quietly and gradually becomes more energetic. The conductor asked the strings to listen closely to the flute instead of playing faster. During the break, two older students helped a beginner mark difficult passages in her score. The concert will be free, although visitors may donate to a fund that buys instruments for children who cannot afford their own.",
            "On the coast, researchers are observing birds that stop during their seasonal migration. They count the birds from a distance and avoid walking through the nesting area. A weather station records the wind speed, temperature, and rainfall throughout the day. The team compares these observations with records collected over the previous decade. Their report will help the council decide where to place a new footpath while protecting the wetland that provides food and shelter for many species.",
            "A family restaurant near the harbor has changed its menu to reduce food waste. The cook now prepares smaller batches and asks customers whether they would prefer a large or a small serving. Vegetables that remain fresh at closing time are used in the next day's soup. Staff measure the amount of discarded food each week and discuss the results at their meeting. After two months, the restaurant was buying fewer ingredients while serving the same number of meals.",
            "The neighborhood newspaper is preparing a feature about all of these projects. Its editor wants readers to understand how ordinary decisions can improve public spaces over time. The reporters have checked names, dates, and quotations with the people they interviewed. A photographer will return in the morning to take pictures of the library entrance and the community garden. Before publication, the team will read every page once more and correct any remaining mistakes. The final report will be ready tomorrow.",
        ]
        .join("\n\n")
    }

    #[cfg(windows)]
    fn peak_working_set_megabytes() -> usize {
        #[repr(C)]
        struct ProcessMemoryCounters {
            size: u32,
            page_fault_count: u32,
            peak_working_set: usize,
            working_set: usize,
            quota_peak_paged_pool: usize,
            quota_paged_pool: usize,
            quota_peak_non_paged_pool: usize,
            quota_non_paged_pool: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        #[link(name = "psapi")]
        unsafe extern "system" {
            fn GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                counters: *mut ProcessMemoryCounters,
                size: u32,
            ) -> i32;
        }
        let mut counters: ProcessMemoryCounters = unsafe { std::mem::zeroed() };
        counters.size = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let result = unsafe {
            GetProcessMemoryInfo(
                -1_isize as *mut _,
                &mut counters,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            )
        };
        assert_ne!(result, 0);
        counters.peak_working_set / (1024 * 1024)
    }

    #[test]
    #[ignore = "requires the downloaded local ONNX runtime and both models; translates over 5000 characters"]
    fn real_long_documents_translate_both_directions_and_remain_cancellable() {
        let english = long_english_passage();
        assert!(english.chars().count() > 5000);
        let started = Instant::now();
        let chinese = translate(&english, "en", "zh").unwrap();
        println!(
            "long en->zh: source_chars={}, target_chars={}, paragraphs={}, elapsed_ms={}",
            english.chars().count(),
            chinese.chars().count(),
            chinese.split("\n\n").count(),
            started.elapsed().as_millis()
        );
        assert_eq!(
            chinese.matches("\n\n").count(),
            english.matches("\n\n").count()
        );
        assert!(chinese.chars().count() > english.chars().count() / 6);
        assert!(chinese.split("\n\n").next().unwrap().contains("图书馆"));
        let final_paragraph = chinese.rsplit("\n\n").next().unwrap();
        assert!(final_paragraph.contains("报告") && final_paragraph.contains("明天"));

        let one_paragraph = english.split("\n\n").take(2).collect::<Vec<_>>().join(" ");
        assert!(one_paragraph.chars().count() > 772);
        let started = Instant::now();
        let translated = translate(&one_paragraph, "en", "zh").unwrap();
        assert!(translated.chars().count() > 100);
        assert!(!translated.contains('\n'));
        println!(
            "single long paragraph en->zh: source_chars={}, target_chars={}, elapsed_ms={}",
            one_paragraph.chars().count(),
            translated.chars().count(),
            started.elapsed().as_millis()
        );

        let chinese_source = "图书馆每天上午九点开门。星期一，几位志愿者提前来到这里，准备新的阅览室。他们把小桌子搬到窗户旁边，把儿童图书放在较低的书架上。一位老师带来了本地区的地图，另一位志愿者修好了入口上方的时钟。中午以前，所有椅子都已摆好，第一批读者正在门外等候。图书管理员感谢大家的帮助，并介绍了免费借书的方法。\r\n\r\n学校的科学课正在研究水的运动。学生把同样多的水倒进不同的容器，记录水面的高度。老师请他们解释，为什么宽碗里的水看起来比窄玻璃杯里的水浅。有一组画了仔细的示意图，另一组重新检查测量结果。实验结束以后，大家清理桌面，并写下希望在下一节课中研究的问题。\r\n\r\n社区报纸正在介绍这些项目。编辑希望读者了解，人们怎样通过日常决定改善公共空间。记者已经核对采访对象的姓名、日期和引用内容。摄影师将在早晨拍摄图书馆入口和社区花园。出版以前，团队会再次检查每一页，纠正剩余的错误。最后一份报告将在明天完成。";
        let started = Instant::now();
        let translated = translate(chinese_source, "zh", "en").unwrap();
        println!(
            "long zh->en: source_chars={}, target_chars={}, elapsed_ms={}",
            chinese_source.chars().count(),
            translated.chars().count(),
            started.elapsed().as_millis()
        );
        assert_eq!(translated.matches("\r\n\r\n").count(), 2);
        let lower = translated.to_lowercase();
        assert!(lower.split("\r\n\r\n").next().unwrap().contains("library"));
        assert!(
            lower
                .rsplit("\r\n\r\n")
                .next()
                .unwrap()
                .contains("tomorrow")
        );

        use std::sync::atomic::{AtomicBool, Ordering};
        let cancelled = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(150));
                cancelled.store(true, Ordering::Release);
            });
            let started = Instant::now();
            let error =
                translate_cancellable(&english, "en", "zh", &|| cancelled.load(Ordering::Acquire))
                    .unwrap_err();
            assert!(error.to_string().contains("取消"), "{error}");
            assert!(started.elapsed() < Duration::from_secs(3));
            println!(
                "cancelled long inference after {} ms",
                started.elapsed().as_millis()
            );
        });
        assert!(
            !translate("The library opens tomorrow.", "en", "zh")
                .unwrap()
                .is_empty()
        );
        #[cfg(windows)]
        println!(
            "offline test process peak working set: {} MiB",
            peak_working_set_megabytes()
        );
    }
    #[test]
    #[ignore = "requires the downloaded local ONNX runtime and models"]
    fn real_abdominal_muscles_translation_keeps_the_complete_word() {
        let result = translate("snake male abs", "en", "zh").unwrap();
        println!("snake male abs => {result}");
        assert!(
            result.contains('肌'),
            "the model ended the word early: {result}"
        );
    }
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
