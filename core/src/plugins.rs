//! Plugins: programs in the workspace's `plugins/<id>/` folder that a pipeline's plugin step
//! runs on each document.
//!
//! A plugin folder holds a `plugin.toml`:
//!
//! ```toml
//! name = "Word list filter"
//! description = "Drops documents with too many listed words."
//! command = ["python", "main.py"]   # run inside the plugin folder
//! workers = 4                       # processes started per step (optional)
//!
//! [[options]]
//! key = "max_hits"
//! default = "0"
//! help = "Allowed matches."
//! ```
//!
//! The protocol is JSON lines over stdin/stdout. For each document the app writes
//! `{"text": "..."}` and the plugin answers with one line:
//! `{"keep": false, "reason": "..."}` to drop it, `{"text": "..."}` to replace its text, or
//! `{}` / `{"keep": true}` to keep it unchanged. Options (defaults merged with the step's
//! `key=value` lines) arrive as a JSON object in the `AEON_PLUGIN_OPTIONS` environment variable.
//! Anything the plugin prints to stderr goes to the app log.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use aeon_shared::{PluginInfo, PluginOption, PluginStep};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::pipeline::Abort;
use crate::pipeline::steps::{Step, Tags, Verdict};

pub const MANIFEST: &str = "plugin.toml";

/// Most processes one plugin step starts.
const MAX_WORKERS: usize = 32;

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    command: Vec<String>,
    #[serde(default)]
    workers: Option<u64>,
    #[serde(default)]
    options: Vec<PluginOption>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn read_manifest(dir: &Path) -> AppResult<Manifest> {
    let m: Manifest = toml::from_str(&std::fs::read_to_string(dir.join(MANIFEST))?)?;
    if m.command.is_empty() || m.command[0].trim().is_empty() {
        return Err(AppError::InvalidInput("plugin.toml: `command` is empty".into()));
    }
    Ok(m)
}

/// Plugins in `dir`, sorted by name. Folders whose plugin.toml can't be read are listed with
/// the error, so the user can see what is wrong.
pub fn list_plugins(dir: &Path) -> AppResult<Vec<PluginInfo>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().join(MANIFEST).is_file() || !valid_id(&id) {
            continue;
        }
        out.push(match read_manifest(&entry.path()) {
            Ok(m) => PluginInfo {
                id,
                name: m.name,
                description: m.description,
                command: m.command,
                options: m.options,
                error: None,
            },
            Err(e) => PluginInfo {
                name: id.clone(),
                id,
                description: None,
                command: Vec::new(),
                options: Vec::new(),
                error: Some(e.to_string()),
            },
        });
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    Ok(out)
}

/// How to start one plugin process.
struct Launch {
    program: PathBuf,
    args: Vec<String>,
    dir: PathBuf,
    options_json: String,
}

impl Launch {
    fn spawn(&self, name: &str) -> std::io::Result<Proc> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .current_dir(&self.dir)
            .env("AEON_PLUGIN_OPTIONS", &self.options_json)
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window flashing up when the desktop app starts a plugin.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        if let Some(stderr) = child.stderr.take() {
            let name = name.to_string();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::warn!("plugin {name}: {line}");
                }
            });
        }
        Ok(Proc { child, stdin, stdout, line: String::new() })
    }
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    line: String,
}

impl Proc {
    fn ask(&mut self, request: &str) -> std::io::Result<String> {
        self.stdin.write_all(request.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        self.line.clear();
        if self.stdout.read_line(&mut self.line)? == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the plugin exited"));
        }
        Ok(std::mem::take(&mut self.line))
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default = "yes")]
    keep: bool,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    text: Option<String>,
    /// Metadata fields to save on the document (kept documents only).
    #[serde(default)]
    tags: serde_json::Map<String, serde_json::Value>,
}

fn yes() -> bool {
    true
}

/// A compiled plugin step: a small pool of plugin processes, started on first use.
pub struct PluginRunner {
    name: String,
    launch: Launch,
    procs: Vec<Mutex<Option<Proc>>>,
    next: AtomicUsize,
    abort: std::sync::Arc<Abort>,
}

impl PluginRunner {
    pub fn new(plugins_dir: Option<&Path>, cfg: &PluginStep, abort: std::sync::Arc<Abort>) -> AppResult<Self> {
        let id = cfg.plugin.trim();
        if id.is_empty() {
            return Err(AppError::InvalidInput("Choose a plugin (Settings → Plugins lists them)".into()));
        }
        if !valid_id(id) {
            return Err(AppError::InvalidInput(format!("Invalid plugin name: {id:?}")));
        }
        let plugins_dir = plugins_dir.ok_or_else(|| AppError::InvalidInput("Plugins need an open workspace".into()))?;
        let dir = plugins_dir.join(id);
        if !dir.join(MANIFEST).is_file() {
            return Err(AppError::InvalidInput(format!(
                "Plugin \"{id}\" is not installed: no {} in {}",
                MANIFEST,
                dir.display()
            )));
        }
        let m = read_manifest(&dir).map_err(|e| AppError::InvalidInput(format!("Plugin \"{id}\": {e}")))?;

        let mut options: BTreeMap<String, String> =
            m.options.iter().map(|o| (o.key.clone(), o.default.clone())).collect();
        for line in cfg.options.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| AppError::InvalidInput(format!("Option \"{line}\" should look like key=value")))?;
            options.insert(k.trim().to_string(), v.trim().to_string());
        }

        // A command naming a file in the plugin folder runs that file; otherwise it is looked
        // up on PATH (python, node, …).
        let first = &m.command[0];
        let local = dir.join(first);
        let program = if (first.contains('/') || first.contains('\\') || first.contains('.')) && local.is_file() {
            local
        } else {
            PathBuf::from(first)
        };
        let workers = m
            .workers
            .map(|w| w as usize)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get()).min(4))
            .clamp(1, MAX_WORKERS);
        Ok(Self {
            name: id.to_string(),
            launch: Launch {
                program,
                args: m.command[1..].to_vec(),
                dir,
                options_json: serde_json::to_string(&options)?,
            },
            procs: (0..workers).map(|_| Mutex::new(None)).collect(),
            next: AtomicUsize::new(0),
            abort,
        })
    }

    /// Starts one process and checks it answers, so a broken plugin fails before a run starts.
    pub fn check(&self) -> AppResult<()> {
        let mut slot = self.procs[0].lock().map_err(|_| AppError::Internal("plugin lock poisoned".into()))?;
        let mut proc = self.launch.spawn(&self.name).map_err(|e| {
            AppError::InvalidInput(format!(
                "Could not start plugin \"{}\" ({}): {e}",
                self.name,
                self.launch.program.display()
            ))
        })?;
        let answer = proc
            .ask(r#"{"text":"AeonDataStudio plugin check."}"#)
            .map_err(|e| AppError::InvalidInput(format!("Plugin \"{}\" did not answer: {e}", self.name)))?;
        serde_json::from_str::<Answer>(&answer).map_err(|e| {
            AppError::InvalidInput(format!("Plugin \"{}\" answered something that is not valid: {e}", self.name))
        })?;
        *slot = Some(proc);
        Ok(())
    }

    fn ask(&self, text: &str) -> Result<Answer, String> {
        let request = serde_json::to_string(&serde_json::json!({ "text": text })).map_err(|e| e.to_string())?;
        // Take a free process if there is one, else wait for the next in turn.
        let n = self.procs.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        let mut guard = (0..n)
            .find_map(|i| self.procs[(start + i) % n].try_lock().ok())
            .unwrap_or_else(|| self.procs[start % n].lock().unwrap_or_else(|p| p.into_inner()));
        if guard.is_none() {
            *guard = Some(self.launch.spawn(&self.name).map_err(|e| format!("could not start: {e}"))?);
        }
        let proc = guard.as_mut().expect("just started");
        match proc.ask(&request) {
            Ok(line) => serde_json::from_str(&line).map_err(|e| format!("invalid answer {:?}: {e}", truncate(&line))),
            Err(e) => {
                *guard = None;
                Err(e.to_string())
            }
        }
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(200).collect()
}

impl Step for PluginRunner {
    fn apply(&self, text: &mut String) -> Verdict {
        self.apply_tagged(text, &mut Tags::new())
    }

    fn apply_tagged(&self, text: &mut String, tags: &mut Tags) -> Verdict {
        if self.abort.is_set() {
            return Verdict::Unchanged;
        }
        // A plugin failing mid-run stops the run with the plugin's error.
        let answer = match self.ask(text) {
            Ok(a) => a,
            Err(e) => {
                self.abort.fail(format!("Plugin \"{}\" failed: {e}", self.name));
                return Verdict::Unchanged;
            }
        };
        tags.extend(answer.tags);
        if !answer.keep {
            return Verdict::Drop {
                rule: "plugin",
                detail: answer.reason.unwrap_or_else(|| format!("dropped by {}", self.name)),
            };
        }
        match answer.text {
            Some(new) if new != *text => {
                *text = new;
                Verdict::Changed
            }
            _ => Verdict::Unchanged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python() -> Option<&'static str> {
        ["python", "python3"].into_iter().find(|p| Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
    }

    fn install(dir: &Path, id: &str, toml: &str, script: &str) {
        let p = dir.join(id);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join(MANIFEST), toml).unwrap();
        std::fs::write(p.join("main.py"), script).unwrap();
    }

    const SCRIPT: &str = r#"
import json, os, sys
opts = json.loads(os.environ["AEON_PLUGIN_OPTIONS"])
word = opts["word"]
for line in sys.stdin:
    text = json.loads(line)["text"]
    if word in text:
        print(json.dumps({"keep": False, "reason": "contains " + word}), flush=True)
    elif "shout" in text:
        print(json.dumps({"text": text.upper()}), flush=True)
    elif "score" in text:
        print(json.dumps({"tags": {"quality": 0.75}}), flush=True)
    else:
        print("{}", flush=True)
"#;

    #[test]
    fn lists_plugins_and_reports_broken_ones() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "good", "name = \"Good\"\ncommand = [\"python\", \"main.py\"]\n[[options]]\nkey = \"word\"\ndefault = \"spam\"\n", "");
        install(dir.path(), "broken", "name = 5", "");
        let list = list_plugins(dir.path()).unwrap();
        assert_eq!(list.len(), 2);
        let good = list.iter().find(|p| p.id == "good").unwrap();
        assert_eq!(good.options[0].default, "spam");
        assert!(list.iter().find(|p| p.id == "broken").unwrap().error.is_some());
    }

    #[test]
    fn runs_a_python_plugin_that_drops_and_rewrites() {
        let Some(py) = python() else {
            eprintln!("python not found; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let toml = format!(
            "name = \"Word\"\ncommand = [\"{py}\", \"main.py\"]\nworkers = 2\n[[options]]\nkey = \"word\"\ndefault = \"spam\"\n"
        );
        install(dir.path(), "word", &toml, SCRIPT);
        let cfg = PluginStep { plugin: "word".into(), options: vec!["word = casino".into()] };
        let runner = PluginRunner::new(Some(dir.path()), &cfg, Default::default()).unwrap();
        runner.check().unwrap();

        let mut t = "visit the casino now".to_string();
        assert!(matches!(runner.apply(&mut t), Verdict::Drop { rule: "plugin", ref detail } if detail == "contains casino"));
        let mut t = "spam is fine here".to_string();
        assert!(matches!(runner.apply(&mut t), Verdict::Unchanged), "option overrides the default");
        let mut t = "please shout".to_string();
        assert!(matches!(runner.apply(&mut t), Verdict::Changed));
        assert_eq!(t, "PLEASE SHOUT");
        let mut tags = Tags::new();
        let mut t = "score me".to_string();
        assert!(matches!(runner.apply_tagged(&mut t, &mut tags), Verdict::Unchanged));
        assert_eq!(tags, vec![("quality".to_string(), serde_json::json!(0.75))]);

        // Used from many threads at once.
        use rayon::prelude::*;
        let kept = (0..200)
            .into_par_iter()
            .filter(|i| {
                let mut t = if i % 2 == 0 { format!("casino {i}") } else { format!("ok {i}") };
                !matches!(runner.apply(&mut t), Verdict::Drop { .. })
            })
            .count();
        assert_eq!(kept, 100);
    }

    #[test]
    fn missing_plugin_and_bad_options_are_clear_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = PluginRunner::new(Some(dir.path()), &PluginStep { plugin: "nope".into(), options: vec![] }, Default::default()).err().unwrap();
        assert!(err.to_string().contains("not installed"), "{err}");
        install(dir.path(), "p", "name = \"P\"\ncommand = [\"python\"]\n", "");
        let err = PluginRunner::new(Some(dir.path()), &PluginStep { plugin: "p".into(), options: vec!["oops".into()] }, Default::default())
            .err()
            .unwrap();
        assert!(err.to_string().contains("key=value"), "{err}");
        assert!(PluginRunner::new(Some(dir.path()), &PluginStep { plugin: "../x".into(), options: vec![] }, Default::default()).is_err());
    }
}
