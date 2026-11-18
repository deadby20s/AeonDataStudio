//! Language-model steps: score or rewrite documents through any OpenAI-compatible
//! `/chat/completions` endpoint (Ollama, llama.cpp, LM Studio, vLLM, hosted APIs).
//!
//! The API key is read from the environment variable the step names; it is never saved.
//! Requests are retried with backoff; what happens after that is the step's `on_error`.
//!
//! Steps ask the server not to "think" (reasoning models would otherwise spend the token
//! budget on hidden reasoning): `chat_template_kwargs.enable_thinking = false` (llama.cpp,
//! vLLM) and `reasoning_effort = "none"`. A server that rejects these fields gets the request
//! again without them, and they are not sent to it again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use aeon_shared::{LlmOnError, LlmRewrite, LlmScore, ScoreMode};
use regex::Regex;
use serde_json::{Value, json};

use crate::error::{AppError, AppResult};
use crate::pipeline::Abort;
use crate::pipeline::steps::{Step, Tags, Verdict};

const RETRIES: u32 = 3;
const TIMEOUT: Duration = Duration::from_secs(180);

/// Answers saved in the workspace (`cache/llm.sqlite`), keyed by everything that decides the
/// answer: URL, model, token limit, temperature and the full prompt with the document. Shared
/// by the steps of one run; other processes (the CLI, the API) may use it at the same time.
pub struct LlmCache {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl LlmCache {
    pub fn open(path: &std::path::Path) -> AppResult<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS answers (
                 key TEXT PRIMARY KEY,
                 answer TEXT NOT NULL,
                 created_ms INTEGER NOT NULL
             );",
        )?;
        Ok(Self { conn: std::sync::Mutex::new(conn) })
    }

    fn key(parts: &[&str]) -> String {
        let mut joined = String::from("v1");
        for p in parts {
            joined.push('\u{0}');
            joined.push_str(p);
        }
        format!("{:032x}", xxhash_rust::xxh3::xxh3_128(joined.as_bytes()))
    }

    fn get(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().ok()?;
        conn.query_row("SELECT answer FROM answers WHERE key = ?1", [key], |r| r.get(0)).ok()
    }

    fn put(&self, key: &str, answer: &str) {
        if let Ok(conn) = self.conn.lock()
            && let Err(e) = conn.execute(
                "INSERT OR REPLACE INTO answers (key, answer, created_ms) VALUES (?1, ?2, ?3)",
                rusqlite::params![key, answer, crate::error::now_ms()],
            )
        {
            log::warn!("could not save an LLM answer in the cache: {e}");
        }
    }
}

pub struct LlmClient {
    cache: Option<Arc<LlmCache>>,
    agent: ureq::Agent,
    url: String,
    model: String,
    api_key: Option<String>,
    max_input_chars: usize,
    /// Cleared when the server rejects the no-thinking fields.
    send_no_think: AtomicBool,
}

impl LlmClient {
    pub fn new(endpoint: &str, model: &str, api_key_env: &str, max_input_chars: u64) -> AppResult<Self> {
        let endpoint = endpoint.trim().trim_end_matches('/');
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            return Err(AppError::InvalidInput(format!(
                "Server URL must start with http:// or https:// (got {endpoint:?})"
            )));
        }
        if model.trim().is_empty() {
            return Err(AppError::InvalidInput("Choose a model".into()));
        }
        let api_key = match api_key_env.trim() {
            "" => None,
            var => Some(std::env::var(var).map_err(|_| {
                AppError::InvalidInput(format!(
                    "The environment variable {var} (API key) is not set. Set it and restart the app."
                ))
            })?),
        };
        if max_input_chars == 0 {
            return Err(AppError::InvalidInput("Max characters sent must be more than 0".into()));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        let url = if endpoint.ends_with("/chat/completions") {
            endpoint.to_string()
        } else {
            format!("{endpoint}/chat/completions")
        };
        Ok(Self {
            cache: None,
            agent,
            url,
            model: model.trim().to_string(),
            api_key,
            max_input_chars: max_input_chars as usize,
            send_no_think: AtomicBool::new(true),
        })
    }

    /// Fills `{text}` in the prompt with the (possibly cut) document.
    pub fn render(&self, prompt: &str, text: &str) -> String {
        let cut: String = match text.char_indices().nth(self.max_input_chars) {
            Some((i, _)) => text[..i].to_string(),
            None => text.to_string(),
        };
        if prompt.contains("{text}") { prompt.replace("{text}", &cut) } else { format!("{prompt}\n\n{cut}") }
    }

    /// One chat completion, retried on network errors, 429 and 5xx.
    /// One chat completion, from the cache when it has the answer.
    pub fn complete(&self, prompt: &str, max_tokens: u64, temperature: f64) -> Result<String, String> {
        let Some(cache) = &self.cache else {
            return self.request(prompt, max_tokens, temperature);
        };
        let key = LlmCache::key(&[&self.url, &self.model, &max_tokens.to_string(), &temperature.to_string(), prompt]);
        if let Some(answer) = cache.get(&key) {
            return Ok(answer);
        }
        let answer = self.request(prompt, max_tokens, temperature)?;
        cache.put(&key, &answer);
        Ok(answer)
    }

    fn request(&self, prompt: &str, max_tokens: u64, temperature: f64) -> Result<String, String> {
        let mut last = String::new();
        let mut attempt = 0;
        while attempt <= RETRIES {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(500 * 2u64.pow(attempt)));
            }
            let no_think = self.send_no_think.load(Ordering::Relaxed);
            let mut body = json!({
                "model": self.model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": max_tokens,
                "temperature": temperature,
                "stream": false,
            });
            if no_think {
                body["chat_template_kwargs"] = json!({ "enable_thinking": false });
                body["reasoning_effort"] = json!("none");
            }
            let mut req = self.agent.post(&self.url);
            if let Some(key) = &self.api_key {
                req = req.header("Authorization", &format!("Bearer {key}"));
            }
            match req.send_json(&body) {
                Ok(mut resp) => {
                    let status = resp.status().as_u16();
                    let text = resp.body_mut().read_to_string().unwrap_or_default();
                    if status == 200 {
                        return parse_content(&text, max_tokens);
                    }
                    last = format!("HTTP {status}: {}", text.chars().take(300).collect::<String>());
                    if no_think && (status == 400 || status == 422) {
                        // Probably the no-thinking fields; try once more without them.
                        self.send_no_think.store(false, Ordering::Relaxed);
                        continue;
                    }
                    if !(status == 429 || status >= 500) {
                        break;
                    }
                }
                Err(e) => last = e.to_string(),
            }
            attempt += 1;
        }
        Err(last)
    }
}

fn parse_content(body: &str, max_tokens: u64) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("invalid response: {e}"))?;
    let choice = &v["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("");
    let answer = strip_thinking(content).trim().to_string();
    if answer.is_empty() {
        return Err(if choice["finish_reason"] == "length" {
            format!(
                "the model used all {max_tokens} answer tokens without answering (a reasoning model that is thinking?). Raise the token limit or use a model without thinking."
            )
        } else {
            format!("empty answer: {}", body.chars().take(300).collect::<String>())
        });
    }
    Ok(answer)
}

/// Reasoning models may put their thoughts in `<think>…</think>` before the answer.
fn strip_thinking(s: &str) -> String {
    static THINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<think>.*?</think>").expect("valid"));
    THINK.replace_all(s, "").into_owned()
}

/// The first number in an answer ("Score: 4" → 4).
pub fn first_number(s: &str) -> Option<f64> {
    static NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-?\d+(?:\.\d+)?").expect("valid"));
    NUM.find(s).and_then(|m| m.as_str().parse().ok())
}

fn on_error(policy: LlmOnError, abort: &Abort, step: &str, e: String) -> Verdict {
    match policy {
        LlmOnError::Stop => {
            abort.fail(format!("{step}: the language model request failed: {e}"));
            Verdict::Unchanged
        }
        LlmOnError::Keep => {
            log::warn!("{step}: request failed, document kept: {e}");
            Verdict::Unchanged
        }
        LlmOnError::Drop => Verdict::Drop { rule: "llm_error", detail: e },
    }
}

pub struct ScoreStep {
    client: LlmClient,
    cfg: LlmScore,
    abort: Arc<Abort>,
}

impl ScoreStep {
    pub fn new(cfg: &LlmScore, abort: Arc<Abort>) -> AppResult<Self> {
        if !cfg.min_score.is_finite() {
            return Err(AppError::InvalidInput("Min score must be a number".into()));
        }
        Ok(Self {
            client: LlmClient::new(&cfg.endpoint, &cfg.model, &cfg.api_key_env, cfg.max_input_chars)?,
            cfg: cfg.clone(),
            abort,
        })
    }
}

/// Answer tokens for a score: enough for "Score: 4" with some preamble.
const SCORE_TOKENS: u64 = 64;

impl ScoreStep {
    /// Saves and reuses answers in `cache`, when the step's settings allow it.
    pub fn with_cache(mut self, cache: Option<Arc<LlmCache>>) -> Self {
        if self.cfg.use_cache {
            self.client.cache = cache;
        }
        self
    }
}

impl Step for ScoreStep {
    fn apply(&self, text: &mut String) -> Verdict {
        self.apply_tagged(text, &mut Tags::new())
    }

    fn apply_tagged(&self, text: &mut String, tags: &mut Tags) -> Verdict {
        if self.abort.is_set() {
            return Verdict::Unchanged;
        }
        let prompt = self.client.render(&self.cfg.prompt, text);
        match self.client.complete(&prompt, SCORE_TOKENS, 0.0) {
            Ok(answer) => match first_number(&answer) {
                Some(score) if self.cfg.mode == ScoreMode::Tag => {
                    tags.push((self.cfg.score_field.clone(), json!(score)));
                    Verdict::Unchanged
                }
                Some(score) if score < self.cfg.min_score => Verdict::Drop {
                    rule: "llm_score_below",
                    detail: format!("score {score} < {}", self.cfg.min_score),
                },
                Some(_) => Verdict::Unchanged,
                None => on_error(self.cfg.on_error, &self.abort, "LLM quality score", format!("no score in answer {answer:?}")),
            },
            Err(e) => on_error(self.cfg.on_error, &self.abort, "LLM quality score", e),
        }
    }
}

pub struct RewriteStep {
    client: LlmClient,
    cfg: LlmRewrite,
    abort: Arc<Abort>,
}

impl RewriteStep {
    pub fn new(cfg: &LlmRewrite, abort: Arc<Abort>) -> AppResult<Self> {
        if !(0.0..=2.0).contains(&cfg.temperature) {
            return Err(AppError::InvalidInput("Temperature must be between 0 and 2".into()));
        }
        if cfg.max_tokens == 0 {
            return Err(AppError::InvalidInput("Max answer tokens must be more than 0".into()));
        }
        Ok(Self {
            client: LlmClient::new(&cfg.endpoint, &cfg.model, &cfg.api_key_env, cfg.max_input_chars)?,
            cfg: cfg.clone(),
            abort,
        })
    }
}

impl RewriteStep {
    /// Saves and reuses answers in `cache`, when the step's settings allow it.
    pub fn with_cache(mut self, cache: Option<Arc<LlmCache>>) -> Self {
        if self.cfg.use_cache {
            self.client.cache = cache;
        }
        self
    }
}

impl Step for RewriteStep {
    fn apply(&self, text: &mut String) -> Verdict {
        if self.abort.is_set() {
            return Verdict::Unchanged;
        }
        let prompt = self.client.render(&self.cfg.prompt, text);
        match self.client.complete(&prompt, self.cfg.max_tokens, self.cfg.temperature) {
            Ok(answer) if answer != *text => {
                *text = answer;
                Verdict::Changed
            }
            Ok(_) => Verdict::Unchanged,
            Err(e) => on_error(self.cfg.on_error, &self.abort, "LLM rewrite", e),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// A fake OpenAI-compatible server. `answer` maps the prompt to (status, content).
    pub fn fake_server(answer: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> (String, Arc<AtomicU64>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let calls = Arc::new(AtomicU64::new(0));
        let counter = calls.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                counter.fetch_add(1, Ordering::Relaxed);
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let prompt = v["messages"][0]["content"].as_str().unwrap_or("").to_string();
                let (status, content) = answer(&prompt);
                let resp = json!({"choices": [{"message": {"role": "assistant", "content": content}}]});
                let _ = req.respond(tiny_http::Response::from_string(resp.to_string()).with_status_code(status));
            }
        });
        (format!("http://127.0.0.1:{port}/v1"), calls)
    }


    #[test]
    fn scores_and_rewrites_through_an_openai_compatible_server() {
        let (url, _) = fake_server(|p| {
            if p.contains("rate") {
                (200, if p.contains("great") { "<think>hmm</think> Score: 4".into() } else { "1".into() })
            } else {
                (200, format!("REWRITTEN {}", p.len()))
            }
        });
        let score = ScoreStep::new(
            &LlmScore { endpoint: url.clone(), prompt: "Please rate: {text}".into(), ..LlmScore::default() },
            Default::default(),
        )
        .unwrap();
        let mut t = "a great text".to_string();
        assert!(matches!(score.apply(&mut t), Verdict::Unchanged));
        let mut t = "junk".to_string();
        assert!(matches!(score.apply(&mut t), Verdict::Drop { rule: "llm_score_below", .. }));

        let rewrite = RewriteStep::new(
            &LlmRewrite { endpoint: url, prompt: "Rewrite: {text}".into(), ..LlmRewrite::default() },
            Default::default(),
        )
        .unwrap();
        let mut t = "hello".to_string();
        assert!(matches!(rewrite.apply(&mut t), Verdict::Changed));
        assert_eq!(t, format!("REWRITTEN {}", "Rewrite: hello".len()));
    }

    #[test]
    fn retries_server_errors_then_follows_on_error() {
        let (url, calls) = fake_server(|_| (503, String::new()));
        let step =
            ScoreStep::new(&LlmScore { endpoint: url.clone(), on_error: LlmOnError::Drop, ..LlmScore::default() }, Default::default())
                .unwrap();
        let mut t = "x".to_string();
        assert!(matches!(step.apply(&mut t), Verdict::Drop { rule: "llm_error", .. }));
        assert_eq!(calls.load(Ordering::Relaxed), u64::from(RETRIES) + 1);

        // With "stop", the first failure aborts and later documents are not sent.
        let abort: Arc<Abort> = Default::default();
        let step = ScoreStep::new(&LlmScore { endpoint: url, ..LlmScore::default() }, abort.clone()).unwrap();
        assert!(matches!(step.apply(&mut t), Verdict::Unchanged));
        assert!(abort.error().unwrap().contains("HTTP 503"));
        let before = calls.load(Ordering::Relaxed);
        assert!(matches!(step.apply(&mut t), Verdict::Unchanged));
        assert_eq!(calls.load(Ordering::Relaxed), before);
    }

    #[test]
    fn retries_without_no_think_fields_when_the_server_rejects_them() {
        let (url, calls) = fake_server(|p| (200, p.to_string()));
        // The fake server answers 400 when it sees the extra fields.
        let _ = url;
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let (status, resp) = if body.contains("reasoning_effort") {
                    (400, json!({"error": "unknown field reasoning_effort"}))
                } else {
                    (200, json!({"choices": [{"message": {"content": "4"}, "finish_reason": "stop"}]}))
                };
                let _ = req.respond(tiny_http::Response::from_string(resp.to_string()).with_status_code(status));
            }
        });
        let step = ScoreStep::new(
            &LlmScore { endpoint: format!("http://127.0.0.1:{port}/v1"), ..LlmScore::default() },
            Default::default(),
        )
        .unwrap();
        let mut t = "x".to_string();
        assert!(matches!(step.apply(&mut t), Verdict::Unchanged));
        assert!(!step.client.send_no_think.load(Ordering::Relaxed));
        let _ = calls;
    }

    #[test]
    fn tag_mode_keeps_documents_and_saves_the_score() {
        let (url, _) = fake_server(|p| (200, if p.contains("great") { "4".into() } else { "1".into() }));
        let step = ScoreStep::new(
            &LlmScore { endpoint: url, mode: ScoreMode::Tag, score_field: "edu".into(), ..LlmScore::default() },
            Default::default(),
        )
        .unwrap();
        let mut tags = Tags::new();
        let mut t = "junk".to_string();
        assert!(matches!(step.apply_tagged(&mut t, &mut tags), Verdict::Unchanged), "kept despite the low score");
        let mut t = "great".to_string();
        assert!(matches!(step.apply_tagged(&mut t, &mut tags), Verdict::Unchanged));
        assert_eq!(tags, vec![("edu".to_string(), json!(1.0)), ("edu".to_string(), json!(4.0))]);
    }

    #[test]
    fn cached_answers_are_reused_across_steps_and_settings_matter() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(LlmCache::open(&dir.path().join("cache").join("llm.sqlite")).unwrap());
        let (url, calls) = fake_server(|p| (200, format!("R:{}", p.len())));
        let make = |prompt: &str, use_cache: bool| {
            RewriteStep::new(
                &LlmRewrite { endpoint: url.clone(), prompt: prompt.into(), use_cache, ..LlmRewrite::default() },
                Default::default(),
            )
            .unwrap()
            .with_cache(Some(cache.clone()))
        };
        let first = make("Rewrite: {text}", true);
        for doc in ["a", "b", "a"] {
            let mut t = doc.to_string();
            first.apply(&mut t);
        }
        assert_eq!(calls.load(Ordering::Relaxed), 2, "the repeated document came from the cache");
        // A new step (a new run) with the same settings asks the server nothing.
        let again = make("Rewrite: {text}", true);
        let mut t = "b".to_string();
        again.apply(&mut t);
        assert_eq!(t, format!("R:{}", "Rewrite: b".len()));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        // A different prompt, or the cache turned off, does ask.
        make("Other: {text}", true).apply(&mut "b".to_string());
        make("Rewrite: {text}", false).apply(&mut "b".to_string());
        assert_eq!(calls.load(Ordering::Relaxed), 4);
        // And the cache survives reopening.
        drop(first);
        drop(again);
        let reopened = Arc::new(LlmCache::open(&dir.path().join("cache").join("llm.sqlite")).unwrap());
        let key = LlmCache::key(&[&format!("{url}/chat/completions"), "llama3.2", "1024", "0.7", "Rewrite: a"]);
        assert_eq!(reopened.get(&key).as_deref(), Some(format!("R:{}", "Rewrite: a".len()).as_str()));
    }

    #[test]
    fn explains_an_answer_cut_off_by_thinking() {
        let body = r#"{"choices":[{"finish_reason":"length","message":{"content":"","reasoning_content":"Hmm"}}]}"#;
        assert!(parse_content(body, 16).unwrap_err().contains("all 16 answer tokens"));
    }

    #[test]
    fn checks_settings_and_cuts_long_documents() {
        assert!(LlmClient::new("localhost:11434", "m", "", 10).is_err());
        assert!(LlmClient::new("http://x", "", "", 10).is_err());
        assert!(LlmClient::new("http://x", "m", "AEON_SURELY_UNSET_VARIABLE", 10).is_err());
        let c = LlmClient::new("http://x/v1/", "m", "", 5).unwrap();
        assert_eq!(c.url, "http://x/v1/chat/completions");
        assert_eq!(c.render("T: {text}", "abcdefgh"), "T: abcde");
        assert_eq!(first_number("I'd say 3.5 out of 5"), Some(3.5));
        assert_eq!(first_number("none"), None);
    }
}
