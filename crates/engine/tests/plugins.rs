//! Plugins end to end: the engine loads a plugin, formulas call its function, and its commands
//! run as one undoable step. The plugin is this test binary started again with
//! `--serve-plugin` (no libtest harness, so standard output is the protocol's alone).

use std::io::{BufRead, Write};

use gridcraft_engine::Session;
use gridcraft_engine::core::{CellError, CellRef, Value};
use serde_json::{Value as Json, json};

const PLUGIN: &str = "org.gridcraft.test-engine";

fn main() {
    if std::env::args().any(|a| a == "--serve-plugin") {
        serve();
        return;
    }
    let tests: [(&str, fn()); 3] = [
        ("functions_recalculate_when_the_plugin_arrives_and_leaves", functions_recalculate_when_the_plugin_arrives_and_leaves),
        ("plugin_commands_are_one_undo_step", plugin_commands_are_one_undo_step),
        ("a_failing_plugin_command_changes_nothing", a_failing_plugin_command_changes_nothing),
    ];
    for (name, t) in tests {
        t();
        println!("test {name} ... ok");
    }
}

/// The plugin side.
fn serve() {
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Json>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { break };
        let p = &msg["params"];
        let result = match msg["method"].as_str() {
            Some("functions/call") => json!({"value": p["args"][0].as_f64().map(|x| json!(x * 2.0)).unwrap_or(json!({"error": "#VALUE!"}))}),
            Some("commands/run") => {
                let at = p["context"]["active"].as_str().unwrap_or("A1");
                let mut cmds = vec![
                    json!({"command": "cell.set", "params": {"cell": at, "input": "one"}}),
                    json!({"command": "cell.set", "params": {"cell": "C1", "input": "=TESTPLUG.DOUBLE(21)"}}),
                ];
                if p["command"] == "broken" {
                    cmds.push(json!({"command": "no.suchCommand"}));
                }
                json!({"commands": cmds, "message": "done", "result": {"saw": p["context"]["selection"]}})
            }
            _ => json!({}),
        };
        if writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result})).is_err() {
            break;
        }
    }
}

fn load(s: &mut Session) {
    let dir = std::env::temp_dir().join(format!("gridcraft-engine-plugin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().unwrap();
    let manifest = json!({
        "id": PLUGIN, "name": "Engine test", "run": {"command": exe, "args": ["--serve-plugin"]},
        "functions": [{"name": "TESTPLUG.DOUBLE", "minArgs": 1, "maxArgs": 1}],
        "commands": [{"id": "fill", "label": "Fill"}, {"id": "broken", "label": "Broken"}],
    });
    std::fs::write(dir.join("plugin.json"), manifest.to_string()).unwrap();
    let info = s.execute("plugins.load", json!({"path": dir})).unwrap();
    assert_eq!(info["id"], PLUGIN);
}

fn value(s: &Session, a: &str) -> Value {
    s.active().unwrap().wb.active().unwrap().value(CellRef::parse(a).unwrap())
}

fn functions_recalculate_when_the_plugin_arrives_and_leaves() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "=TESTPLUG.DOUBLE(B1)"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "4"})).unwrap();
    assert_eq!(value(&s, "A1"), Value::Error(CellError::Name));
    load(&mut s);
    assert_eq!(value(&s, "A1"), Value::number(8.0));
    s.execute("cell.set", json!({"cell": "B1", "input": "5"})).unwrap();
    assert_eq!(value(&s, "A1"), Value::number(10.0));
    let listed = s.execute("home.addIns", json!({})).unwrap();
    assert!(listed["addIns"].as_array().unwrap().iter().any(|p| p["id"] == PLUGIN));
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
    assert_eq!(value(&s, "A1"), Value::Error(CellError::Name));
    assert!(s.execute("plugins.unload", json!({"id": PLUGIN})).is_err());
}

fn plugin_commands_are_one_undo_step() {
    let mut s = Session::new();
    s.new_workbook();
    load(&mut s);
    s.execute("selection.set", json!({"range": "B2:B3"})).unwrap();
    let undo = s.active().unwrap().undo.len();
    let journal = s.journal.len();
    let r = s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "fill"})).unwrap();
    assert_eq!(r["ran"], 2);
    assert_eq!(r["result"]["saw"], "B2:B3");
    assert_eq!(value(&s, "B2"), Value::from("one"));
    assert_eq!(value(&s, "C1"), Value::number(42.0));
    assert_eq!(s.active().unwrap().undo.len(), undo + 1);
    assert_eq!(s.journal.len(), journal + 1);
    assert_eq!(s.journal.last().unwrap().0, "plugins.run");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(value(&s, "B2"), Value::Empty);
    assert_eq!(value(&s, "C1"), Value::Empty);
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
}

fn a_failing_plugin_command_changes_nothing() {
    let mut s = Session::new();
    s.new_workbook();
    load(&mut s);
    let undo = s.active().unwrap().undo.len();
    let e = s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "broken"})).unwrap_err().to_string();
    assert!(e.contains("nothing was changed"), "{e}");
    assert_eq!(value(&s, "A1"), Value::Empty);
    assert_eq!(s.active().unwrap().undo.len(), undo);
    assert!(s.execute("plugins.run", json!({"plugin": PLUGIN, "command": "nope"})).is_err());
    assert!(s.execute("plugins.run", json!({})).is_err());
    s.execute("plugins.unload", json!({"id": PLUGIN})).unwrap();
}
