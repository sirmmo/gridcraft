//! GridCraft's plugin host.
//!
//! A plugin is a separate program, written in any language, described by a `plugin.json`
//! manifest. GridCraft starts it on first use and talks JSON-RPC 2.0 with it over its standard
//! input and output, one message per line. A plugin contributes:
//! - **functions** that formulas call by name (`=ACME.PRICE("X")`), registered with
//!   [`gridcraft_calc::external`];
//! - **commands** run through `plugins.run`: the plugin answers with engine commands, which the
//!   engine executes as one undoable step.
//!
//! A plugin runs in its own process, so a crash, a hang or a bad answer from it costs a cell an
//! error value, never the workbook. Calls time out; a plugin that keeps failing is stopped.
//! The protocol is documented in `docs/plugins.md`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod process;
pub mod values;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use gridcraft_core::{CellError, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

pub use process::{Connection, RequestError};

/// The protocol version this host speaks (sent in `initialize`).
pub const PROTOCOL_VERSION: u32 = 1;
/// The manifest's file name inside a plugin folder.
pub const MANIFEST: &str = "plugin.json";
/// Environment variable listing extra plugin folders (the platform's path-list separator).
pub const PATH_ENV: &str = "GRIDCRAFT_PLUGIN_PATH";

const MAX_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_DECLARATIONS: usize = 1_000;
/// Default and largest time a function call may take.
const DEFAULT_TIMEOUT_MS: u64 = 2_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
/// Commands get longer: they are run on request, not during recalculation.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const INIT_TIMEOUT: Duration = Duration::from_secs(10);
/// A plugin that crashes or times out this many times is not started again until reloaded.
const MAX_FAILURES: u32 = 3;
/// Results remembered per plugin for non-volatile functions.
const CACHE_LIMIT: usize = 10_000;

// ---------------------------------------------------------------- manifest

/// `plugin.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Unique id, e.g. `org.example.finance` (letters, digits, `.`, `-`, `_`).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub run: RunSpec,
    #[serde(default)]
    pub functions: Vec<FunctionDecl>,
    #[serde(default)]
    pub commands: Vec<CommandDecl>,
    /// Ribbon groups and menu items that run the plugin's commands.
    #[serde(default)]
    pub ui: UiDecl,
}

/// How to start the plugin. A `command` containing a path separator is relative to the plugin
/// folder; a bare name is looked up on `PATH`. The plugin runs in its folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDecl {
    /// The name formulas use, e.g. `ACME.PRICE`. Must not be a built-in function.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Shown in the function list, e.g. `ACME.PRICE(ticker, [date])`.
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub min_args: usize,
    #[serde(default = "max_args_default")]
    pub max_args: usize,
    /// Recalculated on every recalculation (results are not cached).
    #[serde(default)]
    pub volatile: bool,
    /// How long a call may take (default 2 s, at most 30 s).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

fn max_args_default() -> usize {
    255
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDecl {
    /// Unique within the plugin.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Params documentation, like the engine's commands.
    #[serde(default)]
    pub params: String,
}

/// Where the plugin's commands appear in the app.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiDecl {
    /// Groups of buttons on a ribbon tab: an existing tab (`Home`, `Data`, …) or a new one.
    #[serde(default)]
    pub ribbon: Vec<RibbonGroupDecl>,
    /// Items added to a right-click menu.
    #[serde(default)]
    pub menus: Vec<MenuItemDecl>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RibbonGroupDecl {
    /// A built-in tab's name (English, as in [`RIBBON_TABS`]) adds the group at the end of that
    /// tab; any other name makes a new tab after Automate.
    pub tab: String,
    pub group: String,
    pub buttons: Vec<ButtonDecl>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonDecl {
    /// One of the plugin's `commands[].id`.
    pub command: String,
    /// Default: the command's label.
    #[serde(default)]
    pub label: Option<String>,
    /// Default: the command's description.
    #[serde(default)]
    pub tooltip: Option<String>,
    /// An icon name from the app's icon set (see `docs/plugins.md`); unknown names get a default.
    #[serde(default)]
    pub icon: Option<String>,
    /// `large` (default) or `small` (stacked three high).
    #[serde(default)]
    pub size: ButtonSize,
    /// Passed to the command.
    #[serde(default)]
    pub params: Json,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ButtonSize {
    #[default]
    Large,
    Small,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuItemDecl {
    /// Which right-click menu: `cell` (default), `row` or `column` (the headers), `sheetTab`.
    #[serde(default)]
    pub menu: MenuKind,
    pub command: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub params: Json,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MenuKind {
    #[default]
    Cell,
    Row,
    Column,
    SheetTab,
}

/// The built-in ribbon tabs a plugin can add groups to.
pub const RIBBON_TABS: &[&str] = &["Home", "Insert", "Draw", "Page Layout", "Formulas", "Data", "Review", "View", "Automate"];
/// Longest tab, group and label text.
const MAX_LABEL: usize = 64;

fn valid_label(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= MAX_LABEL
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn valid_function_name(s: &str) -> bool {
    s.len() <= 255
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'))
        && !s.ends_with('.')
}

impl Manifest {
    /// Parses and validates a manifest.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let mut m: Manifest = serde_json::from_str(text).map_err(|e| format!("{MANIFEST}: {e}"))?;
        m.validate()?;
        for f in &mut m.functions {
            f.name = f.name.to_ascii_uppercase();
        }
        Ok(m)
    }

    fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!("{MANIFEST}: id `{}` must be 1–128 letters, digits, `.`, `-` or `_`", self.id));
        }
        if self.name.trim().is_empty() {
            return Err(format!("{MANIFEST}: `name` is empty"));
        }
        if self.run.command.trim().is_empty() {
            return Err(format!("{MANIFEST}: `run.command` is empty"));
        }
        if self.functions.len() > MAX_DECLARATIONS || self.commands.len() > MAX_DECLARATIONS {
            return Err(format!("{MANIFEST}: at most {MAX_DECLARATIONS} functions and {MAX_DECLARATIONS} commands"));
        }
        let mut seen = std::collections::HashSet::new();
        for f in &self.functions {
            if !valid_function_name(&f.name) {
                return Err(format!("{MANIFEST}: function name `{}` must start with a letter and hold letters, digits, `.` or `_`", f.name));
            }
            if !seen.insert(f.name.to_ascii_uppercase()) {
                return Err(format!("{MANIFEST}: function `{}` is declared twice", f.name));
            }
            if f.min_args > f.max_args || f.max_args > 255 {
                return Err(format!("{MANIFEST}: function `{}` needs minArgs ≤ maxArgs ≤ 255", f.name));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.commands {
            if !valid_id(&c.id) {
                return Err(format!("{MANIFEST}: command id `{}` must be letters, digits, `.`, `-` or `_`", c.id));
            }
            if !seen.insert(c.id.as_str()) {
                return Err(format!("{MANIFEST}: command `{}` is declared twice", c.id));
            }
        }
        let buttons: usize = self.ui.ribbon.iter().map(|g| g.buttons.len()).sum();
        if self.ui.ribbon.len() > MAX_DECLARATIONS || buttons > MAX_DECLARATIONS || self.ui.menus.len() > MAX_DECLARATIONS {
            return Err(format!("{MANIFEST}: at most {MAX_DECLARATIONS} ribbon groups, buttons and menu items"));
        }
        let known = |id: &str, what: &str| {
            if seen.contains(id) { Ok(()) } else { Err(format!("{MANIFEST}: {what} runs `{id}`, which isn't one of the plugin's commands")) }
        };
        for g in &self.ui.ribbon {
            if !valid_label(&g.tab) || !valid_label(&g.group) {
                return Err(format!("{MANIFEST}: ribbon `tab` and `group` need 1–{MAX_LABEL} characters"));
            }
            if g.buttons.is_empty() {
                return Err(format!("{MANIFEST}: ribbon group `{}` has no buttons", g.group));
            }
            for b in &g.buttons {
                known(&b.command, &format!("a button in ribbon group `{}`", g.group))?;
                if b.label.as_deref().is_some_and(|l| !valid_label(l)) {
                    return Err(format!("{MANIFEST}: button labels need 1–{MAX_LABEL} characters"));
                }
            }
        }
        for item in &self.ui.menus {
            known(&item.command, "a menu item")?;
            if item.label.as_deref().is_some_and(|l| !valid_label(l)) {
                return Err(format!("{MANIFEST}: menu item labels need 1–{MAX_LABEL} characters"));
            }
        }
        Ok(())
    }

    /// The command a button or menu item runs.
    pub fn command(&self, id: &str) -> Option<&CommandDecl> {
        self.commands.iter().find(|c| c.id == id)
    }
}

// ---------------------------------------------------------------- plugins

enum State {
    /// Not started yet, or stopped after a failure (started again on the next call).
    Idle,
    Running(Connection),
    /// Failed too often; not started again until the plugin is reloaded.
    Failed(String),
}

/// A loaded plugin.
pub struct Plugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
    state: Mutex<(State, u32)>,
    cache: Mutex<HashMap<String, Value>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Plugin {
    fn new(manifest: Manifest, dir: PathBuf) -> Plugin {
        Plugin { manifest, dir, state: Mutex::new((State::Idle, 0)), cache: Mutex::new(HashMap::new()) }
    }

    /// Sends a request and waits for its result, starting the plugin when needed.
    fn request(&self, method: &str, params: Json, timeout: Duration) -> Result<Json, String> {
        let mut guard = lock(&self.state);
        let (state, failures) = &mut *guard;
        if let State::Failed(why) = state {
            return Err(why.clone());
        }
        if !matches!(state, State::Running(_)) {
            let mut conn = Connection::start(&self.manifest, &self.dir)?;
            let init = json!({
                "protocolVersion": PROTOCOL_VERSION,
                "host": {"name": "GridCraft", "version": env!("CARGO_PKG_VERSION")},
                "plugin": {"id": self.manifest.id, "dir": self.dir.display().to_string()},
            });
            if let Err(e) = conn.request("initialize", init, INIT_TIMEOUT) {
                *failures += 1;
                *state =
                    if *failures >= MAX_FAILURES { State::Failed(format!("stopped after {failures} failures: {}", e.message)) } else { State::Idle };
                return Err(e.message);
            }
            *state = State::Running(conn);
        }
        let State::Running(conn) = state else { return Err("plugin not running".into()) };
        match conn.request(method, params, timeout) {
            Ok(v) => Ok(v),
            Err(e) if e.fatal => {
                // The connection is gone or out of step: stop the process and start afresh next time.
                *failures += 1;
                log::warn!("plugin {}: {} (failure {failures} of {MAX_FAILURES})", self.manifest.id, e.message);
                *state =
                    if *failures >= MAX_FAILURES { State::Failed(format!("stopped after {failures} failures: {}", e.message)) } else { State::Idle };
                Err(e.message)
            }
            Err(e) => Err(e.message),
        }
    }

    /// Calls one of the plugin's functions. Errors become `#VALUE!` (logged).
    fn call_function(&self, decl: &FunctionDecl, args: &[Value]) -> Value {
        if values::cell_count(args) > values::MAX_CELLS {
            return Value::Error(CellError::Value);
        }
        let args_json: Vec<Json> = args.iter().map(values::to_json).collect();
        let key = (!decl.volatile).then(|| format!("{}\u{0}{}", decl.name, Json::Array(args_json.clone())));
        if let Some(k) = &key
            && let Some(v) = lock(&self.cache).get(k)
        {
            return v.clone();
        }
        let timeout = Duration::from_millis(decl.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1, MAX_TIMEOUT_MS));
        match self.request("functions/call", json!({"name": decl.name, "args": args_json}), timeout) {
            Ok(result) => {
                let v = values::from_json(result.get("value").unwrap_or(&Json::Null));
                if let Some(k) = key {
                    let mut cache = lock(&self.cache);
                    if cache.len() >= CACHE_LIMIT {
                        cache.clear();
                    }
                    cache.insert(k, v.clone());
                }
                v
            }
            Err(e) => {
                log::warn!("plugin {}: {}: {e}", self.manifest.id, decl.name);
                Value::Error(CellError::Value)
            }
        }
    }

    /// `ready` (not started), `running`, or `failed: …`.
    pub fn status(&self) -> String {
        match &lock(&self.state).0 {
            State::Idle => "ready".into(),
            State::Running(_) => "running".into(),
            State::Failed(why) => format!("failed: {why}"),
        }
    }

    /// The plugin as `plugins.list` reports it.
    pub fn info(&self) -> Json {
        let m = &self.manifest;
        json!({
            "id": m.id,
            "name": m.name,
            "version": m.version,
            "description": m.description,
            "dir": self.dir.display().to_string(),
            "status": self.status(),
            "functions": m.functions.iter().map(|f| json!({
                "name": f.name, "signature": f.signature, "description": f.description, "volatile": f.volatile,
            })).collect::<Vec<_>>(),
            "commands": m.commands.iter().map(|c| json!({
                "id": c.id, "label": c.label, "description": c.description, "params": c.params,
            })).collect::<Vec<_>>(),
            "ui": m.ui,
        })
    }

    fn stop(&self) {
        let mut guard = lock(&self.state);
        if let State::Running(conn) = std::mem::replace(&mut guard.0, State::Idle) {
            conn.shutdown();
        }
    }
}

/// A plugin function as the calculation engine sees it.
struct PluginFunction {
    plugin: Arc<Plugin>,
    decl: FunctionDecl,
}

impl gridcraft_calc::external::ExternalFunction for PluginFunction {
    fn call(&self, args: &[Value]) -> Value {
        self.plugin.call_function(&self.decl, args)
    }
    fn volatile(&self) -> bool {
        self.decl.volatile
    }
    fn min_args(&self) -> usize {
        self.decl.min_args
    }
    fn max_args(&self) -> usize {
        self.decl.max_args
    }
}

// ---------------------------------------------------------------- the registry

/// Plugins are application-wide, like Excel's add-ins: one set per process.
fn loaded() -> &'static Mutex<Vec<Arc<Plugin>>> {
    static LOADED: OnceLock<Mutex<Vec<Arc<Plugin>>>> = OnceLock::new();
    LOADED.get_or_init(|| Mutex::new(Vec::new()))
}

/// The plugin with this id.
pub fn get(id: &str) -> Option<Arc<Plugin>> {
    lock(loaded()).iter().find(|p| p.manifest.id == id).cloned()
}

/// Every loaded plugin, in load order.
pub fn list() -> Vec<Arc<Plugin>> {
    lock(loaded()).clone()
}

/// Loads the plugin in `path` (a folder holding `plugin.json`, or the manifest itself). A plugin
/// with the same id is unloaded first. The process starts on first use.
pub fn load(path: &Path) -> Result<Arc<Plugin>, String> {
    let (dir, file) = if path.is_dir() {
        (path.to_path_buf(), path.join(MANIFEST))
    } else {
        (path.parent().unwrap_or(Path::new(".")).to_path_buf(), path.to_path_buf())
    };
    let len = std::fs::metadata(&file).map_err(|e| format!("{}: {e}", file.display()))?.len();
    if len > MAX_MANIFEST_BYTES {
        return Err(format!("{}: manifest larger than 1 MiB", file.display()));
    }
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let manifest = Manifest::parse(&text)?;
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    install(manifest, dir)
}

/// Installs a parsed manifest (tests and [`load`]).
pub fn install(manifest: Manifest, dir: PathBuf) -> Result<Arc<Plugin>, String> {
    let previous = get(&manifest.id);
    for f in &manifest.functions {
        let ours = previous.as_ref().is_some_and(|p| p.manifest.functions.iter().any(|g| g.name == f.name));
        if !ours && gridcraft_calc::is_known_function(&f.name) {
            let owner = list().into_iter().find(|p| p.manifest.functions.iter().any(|g| g.name == f.name));
            return Err(match owner {
                Some(p) => format!("function {} is already provided by plugin {}", f.name, p.manifest.id),
                None => format!("function {} is a built-in function", f.name),
            });
        }
    }
    if previous.is_some() {
        unload(&manifest.id);
    }
    let plugin = Arc::new(Plugin::new(manifest, dir));
    for f in &plugin.manifest.functions {
        gridcraft_calc::external::register(&f.name, Arc::new(PluginFunction { plugin: plugin.clone(), decl: f.clone() }));
    }
    lock(loaded()).push(plugin.clone());
    log::info!("plugin {} {} loaded from {}", plugin.manifest.id, plugin.manifest.version, plugin.dir.display());
    Ok(plugin)
}

/// Unloads a plugin: its functions disappear (formulas using them show `#NAME?` after the next
/// recalculation) and its process is asked to exit. True when it was loaded.
pub fn unload(id: &str) -> bool {
    let plugin = {
        let mut all = lock(loaded());
        let Some(i) = all.iter().position(|p| p.manifest.id == id) else { return false };
        all.remove(i)
    };
    for f in &plugin.manifest.functions {
        gridcraft_calc::external::unregister(&f.name);
    }
    plugin.stop();
    true
}

/// Plugin folders to load at startup: each folder in [`PATH_ENV`], then `extra`. A folder holding
/// `plugin.json` is one plugin; otherwise each of its subfolders holding one is.
pub fn discover(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os(PATH_ENV).map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    roots.extend(extra.iter().cloned());
    let mut out = Vec::new();
    for root in roots {
        if root.join(MANIFEST).is_file() {
            out.push(root);
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join(MANIFEST).is_file()).collect();
        dirs.sort();
        out.extend(dirs);
    }
    out
}

/// Loads every plugin [`discover`] finds; failures are returned (and logged), not fatal.
pub fn autoload(extra: &[PathBuf]) -> Vec<(PathBuf, Result<String, String>)> {
    discover(extra)
        .into_iter()
        .map(|dir| {
            let r = load(&dir).map(|p| p.manifest.id.clone());
            if let Err(e) = &r {
                log::warn!("plugin {}: {e}", dir.display());
            }
            (dir, r)
        })
        .collect()
}

// ---------------------------------------------------------------- commands

/// What a plugin command asks the engine to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandReply {
    /// Engine commands to run, in order: `(id, params)`.
    pub commands: Vec<(String, Json)>,
    /// A message to show the user.
    pub message: Option<String>,
    /// Anything else the plugin returned (handed back to the caller).
    pub result: Json,
}

/// Most engine commands one plugin command may run.
pub const MAX_REPLY_COMMANDS: usize = 10_000;

/// Runs one of a plugin's commands. `context` describes the workbook (sheet, selection, values).
pub fn run_command(plugin_id: &str, command: &str, params: Json, context: Json) -> Result<CommandReply, String> {
    let plugin = get(plugin_id).ok_or_else(|| format!("no plugin `{plugin_id}` is loaded"))?;
    if !plugin.manifest.commands.iter().any(|c| c.id == command) {
        return Err(format!("plugin {plugin_id} has no command `{command}`"));
    }
    let r = plugin.request("commands/run", json!({"command": command, "params": params, "context": context}), COMMAND_TIMEOUT)?;
    parse_reply(r)
}

fn parse_reply(r: Json) -> Result<CommandReply, String> {
    let mut reply = CommandReply {
        message: r.get("message").and_then(Json::as_str).map(str::to_string),
        result: r.get("result").cloned().unwrap_or(Json::Null),
        ..Default::default()
    };
    if let Some(list) = r.get("commands") {
        let list = list.as_array().ok_or("`commands` must be an array")?;
        if list.len() > MAX_REPLY_COMMANDS {
            return Err(format!("a plugin command may run at most {MAX_REPLY_COMMANDS} commands"));
        }
        for c in list {
            let id = c.get("command").and_then(Json::as_str).ok_or("each entry of `commands` needs a `command` id")?;
            reply.commands.push((id.to_string(), c.get("params").cloned().unwrap_or(Json::Null)));
        }
    }
    Ok(reply)
}

#[cfg(test)]
mod tests;
