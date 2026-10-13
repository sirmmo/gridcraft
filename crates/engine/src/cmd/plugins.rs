//! Plugins (Home › Add-ins): load and unload out-of-process plugins, list them, and run their
//! commands. Their functions are called from formulas (see `gridcraft_plugin`).

use std::sync::Arc;

use gridcraft_core::RangeRef;
use serde_json::{Value as Json, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session, UiRequest};

/// Most selected cells whose values a plugin command receives.
const MAX_CONTEXT_CELLS: u64 = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "plugins.list", "Plugins", [], None, "{} → [{id, name, version, description, dir, status, functions, commands}]", always, |_, _| Ok(list())),
        cmd!(query "plugins.load", "Load Plugin", [], None, "{path} → the plugin. `path`: a folder holding plugin.json, or the manifest", always, load),
        cmd!(query "plugins.unload", "Unload Plugin", [], None, "{id}", always, unload),
        cmd!(
            "plugins.run",
            "Run Plugin Command",
            [],
            None,
            "{plugin, command, params?} → {ran, message?, result?}. The plugin's commands run as one undoable step",
            always,
            run
        ),
    ]
}

/// `home.addIns` and `plugins.list`.
pub fn list() -> Json {
    Json::Array(gridcraft_plugin::list().iter().map(|p| p.info()).collect())
}

fn load(s: &mut Session, p: &Json) -> Result<Json> {
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad("plugins.load", "missing `path`"))?;
    let plugin = gridcraft_plugin::load(std::path::Path::new(path)).map_err(|e| bad("plugins.load", e))?;
    recalc_users(s, &plugin.manifest.functions.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
    Ok(plugin.info())
}

fn unload(s: &mut Session, p: &Json) -> Result<Json> {
    let id = str_param(p, "id").ok_or_else(|| bad("plugins.unload", "missing `id`"))?;
    let plugin = gridcraft_plugin::get(id).ok_or_else(|| bad("plugins.unload", format!("no plugin `{id}` is loaded")))?;
    gridcraft_plugin::unload(id);
    recalc_users(s, &plugin.manifest.functions.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
    Ok(json!({"unloaded": id}))
}

/// Recalculates the open workbooks with formulas that call one of `names`, so their cells pick
/// up a plugin that just arrived (or show `#NAME?` for one that left).
fn recalc_users(s: &mut Session, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let calls: Vec<String> = names.iter().map(|n| format!("{}(", n.to_ascii_uppercase())).collect();
    for d in s.docs.iter_mut() {
        let uses = d.wb.sheets.iter().any(|sh| {
            sh.cells.iter().any(|(_, cell)| cell.formula.as_ref().is_some_and(|f| calls.iter().any(|c| f.text.to_ascii_uppercase().contains(c))))
        });
        if uses {
            let wb = Arc::make_mut(&mut d.wb);
            d.calc.recalc_all(wb);
        }
    }
}

/// What a plugin command sees of the workbook: its title, the active sheet, the selection and,
/// for a selection up to 100,000 cells, its values (rows of values, as functions get them).
fn context(s: &Session) -> Json {
    let Some(d) = s.active() else { return json!({}) };
    let sel = &d.selection;
    let range = sel.ranges.last().copied().unwrap_or(RangeRef::cell(sel.active));
    let mut ctx = json!({
        "workbook": d.display_title(),
        "sheet": d.wb.active().map(|sh| sh.name.clone()).unwrap_or_default(),
        "active": sel.active.to_string(),
        "selection": range.to_string(),
    });
    if let Some(sh) = d.wb.active()
        && range.count() <= MAX_CONTEXT_CELLS
    {
        let rows: Vec<Json> = (range.start.row..=range.end.row)
            .map(|r| {
                Json::Array(
                    (range.start.col..=range.end.col)
                        .map(|c| gridcraft_plugin::values::to_json(&sh.value(gridcraft_core::CellRef::new(r, c))))
                        .collect(),
                )
            })
            .collect();
        ctx["values"] = Json::Array(rows);
    }
    ctx
}

fn run(s: &mut Session, p: &Json) -> Result<Json> {
    const ID: &str = "plugins.run";
    let plugin = str_param(p, "plugin").ok_or_else(|| bad(ID, "missing `plugin`"))?;
    let command = str_param(p, "command").ok_or_else(|| bad(ID, "missing `command`"))?;
    let params = p.get("params").cloned().unwrap_or_else(|| json!({}));
    let reply = gridcraft_plugin::run_command(plugin, command, params, context(s)).map_err(|e| bad(ID, e))?;
    if let Some((id, _)) = reply.commands.iter().find(|(id, _)| id.starts_with("plugins.")) {
        return Err(bad(ID, format!("a plugin command can't run `{id}`")));
    }
    // The engine commands run as one step: their own undo and journal entries are dropped (this
    // command's entry covers them), and if one fails the workbook goes back to how it was.
    let before = s.active().map(|d| (d.uid, d.wb.clone(), d.selection.clone(), d.undo.len()));
    let journal_len = s.journal.len();
    let mut failure = None;
    for (id, params) in &reply.commands {
        if let Err(e) = s.execute(id, params.clone()) {
            failure = Some(format!("{id}: {e}"));
            break;
        }
    }
    s.journal.truncate(journal_len);
    if let Some((uid, wb, sel, undo_len)) = before
        && let Some(d) = s.docs.iter_mut().find(|d| d.uid == uid)
    {
        d.undo.truncate(undo_len);
        if failure.is_some() {
            d.wb = wb;
            d.selection = sel;
            d.calc.rebuild(&d.wb);
        }
    }
    if let Some(why) = failure {
        return Err(bad(ID, format!("{plugin} › {command} failed, nothing was changed: {why}")));
    }
    if let Some(m) = &reply.message {
        s.ui_requests.push(UiRequest::Message(m.clone()));
    }
    let mut out = json!({"ran": reply.commands.len()});
    if let Some(m) = reply.message {
        out["message"] = json!(m);
    }
    if !reply.result.is_null() {
        out["result"] = reply.result;
    }
    Ok(out)
}
