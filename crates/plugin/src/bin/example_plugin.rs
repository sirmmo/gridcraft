//! A reference GridCraft plugin, and the one the plugin host's tests drive.
//!
//! ```text
//! gridcraft-example-plugin --manifest > my-plugins/example/plugin.json
//! gridcraft-cli --plugin my-plugins/example eval '=EXAMPLE.HELLO("world")'
//! ```
//!
//! A plugin reads JSON-RPC requests from standard input, one per line, and writes one response
//! line per request to standard output; it exits when its input ends. See `docs/plugins.md`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::io::{BufRead, Write};

use serde_json::{Value as Json, json};

fn manifest() -> Json {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "gridcraft-example-plugin".into());
    json!({
        "id": "org.gridcraft.example",
        "name": "Example plugin",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Reference plugin: a few functions and commands",
        "run": {"command": exe},
        "functions": [
            {"name": "EXAMPLE.HELLO", "signature": "EXAMPLE.HELLO(name)", "description": "Greets name", "minArgs": 1, "maxArgs": 1},
            {"name": "EXAMPLE.SUMSQ", "signature": "EXAMPLE.SUMSQ(number1, ...)", "description": "Sum of the squares of the numbers (ranges too)", "minArgs": 1},
            {"name": "EXAMPLE.COUNTUP", "signature": "EXAMPLE.COUNTUP(n)", "description": "A column 1..n (spills)", "minArgs": 1, "maxArgs": 1},
            {"name": "EXAMPLE.CALLS", "signature": "EXAMPLE.CALLS()", "description": "How many calls this process has answered (volatile)", "maxArgs": 0, "volatile": true},
            {"name": "EXAMPLE.SLEEP", "signature": "EXAMPLE.SLEEP(ms)", "description": "Waits ms milliseconds, then returns ms", "minArgs": 1, "maxArgs": 1, "volatile": true, "timeoutMs": 300},
            {"name": "EXAMPLE.CRASH", "signature": "EXAMPLE.CRASH()", "description": "Exits the plugin process", "maxArgs": 0, "volatile": true}
        ],
        "commands": [
            {"id": "countUp", "label": "Count Up", "description": "Fills 1..count down from the active cell", "params": "{count?: number = 10}"},
            {"id": "reverse", "label": "Reverse Rows", "description": "Reverses the order of the selected rows"},
            {"id": "broken", "label": "Broken", "description": "Writes a cell, then runs a command that fails (the engine undoes both)"}
        ],
        "ui": {
            "ribbon": [
                {"tab": "Example", "group": "Fill", "buttons": [
                    {"command": "countUp", "icon": "plus"},
                    {"command": "countUp", "label": "Count to 3", "size": "small", "params": {"count": 3}},
                    {"command": "reverse", "size": "small", "icon": "sort"}
                ]},
                {"tab": "Data", "group": "Example", "buttons": [{"command": "reverse", "icon": "sortDesc"}]}
            ],
            "menus": [
                {"menu": "cell", "command": "reverse", "label": "Reverse Rows (Example)"},
                {"menu": "row", "command": "countUp", "label": "Count Up Here (Example)", "params": {"count": 5}}
            ]
        }
    })
}

fn main() {
    if std::env::args().any(|a| a == "--manifest") {
        let _ = writeln!(std::io::stdout(), "{}", serde_json::to_string_pretty(&manifest()).unwrap_or_default());
        return;
    }
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut calls = 0u64;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Json>(&line) else { continue };
        let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Json::Null);
        let Some(id) = msg.get("id").cloned() else {
            if method == "shutdown" {
                break;
            }
            continue;
        };
        calls += 1;
        let reply = match method {
            "initialize" => Ok(json!({"protocolVersion": 1})),
            "functions/call" => call(&params, calls),
            "commands/run" => command(&params),
            other => Err(format!("unknown method {other}")),
        };
        let out = match reply {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(message) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": message}}),
        };
        if writeln!(stdout, "{out}").and_then(|_| stdout.flush()).is_err() {
            break;
        }
    }
}

fn numbers(v: &Json, out: &mut Vec<f64>) {
    match v {
        Json::Number(n) => out.extend(n.as_f64()),
        Json::Array(items) => items.iter().for_each(|i| numbers(i, out)),
        _ => {}
    }
}

fn call(p: &Json, calls: u64) -> Result<Json, String> {
    let name = p.get("name").and_then(Json::as_str).unwrap_or("");
    let args = p.get("args").and_then(Json::as_array).cloned().unwrap_or_default();
    let first = args.first().cloned().unwrap_or(Json::Null);
    let value = match name {
        "EXAMPLE.HELLO" => {
            let who = match &first {
                Json::String(s) => s.clone(),
                Json::Null => String::new(),
                other => other.to_string(),
            };
            json!(format!("Hello, {who}!"))
        }
        "EXAMPLE.SUMSQ" => {
            let mut v = Vec::new();
            args.iter().for_each(|a| numbers(a, &mut v));
            json!(v.iter().map(|x| x * x).sum::<f64>())
        }
        "EXAMPLE.COUNTUP" => {
            let n = first.as_f64().unwrap_or(0.0);
            if !(1.0..=100_000.0).contains(&n) { json!({"error": "#NUM!"}) } else { Json::Array((1..=n as u64).map(|i| json!([i])).collect()) }
        }
        "EXAMPLE.CALLS" => json!(calls),
        "EXAMPLE.SLEEP" => {
            let ms = first.as_f64().unwrap_or(0.0).clamp(0.0, 60_000.0);
            std::thread::sleep(std::time::Duration::from_millis(ms as u64));
            json!(ms)
        }
        "EXAMPLE.CRASH" => std::process::exit(3),
        other => return Err(format!("unknown function {other}")),
    };
    Ok(json!({"value": value}))
}

fn command(p: &Json) -> Result<Json, String> {
    let ctx = p.get("context").cloned().unwrap_or(Json::Null);
    let active = ctx.get("active").and_then(Json::as_str).unwrap_or("A1").to_string();
    match p.get("command").and_then(Json::as_str).unwrap_or("") {
        "countUp" => {
            let count = p.get("params").and_then(|q| q.get("count")).and_then(Json::as_u64).unwrap_or(10).clamp(1, 10_000);
            let values: Vec<Json> = (1..=count).map(|i| json!([i])).collect();
            Ok(json!({
                "commands": [{"command": "range.setValues", "params": {"range": active, "values": values}}],
                "message": format!("Wrote 1 to {count}"),
            }))
        }
        "reverse" => {
            let range = ctx.get("selection").and_then(Json::as_str).unwrap_or("A1").to_string();
            let mut rows = ctx.get("values").and_then(Json::as_array).cloned().ok_or("the selection's values are not available")?;
            rows.reverse();
            Ok(json!({"commands": [{"command": "range.setValues", "params": {"range": range, "values": rows}}]}))
        }
        "broken" => Ok(json!({"commands": [
            {"command": "cell.set", "params": {"cell": active, "input": "partial"}},
            {"command": "no.suchCommand", "params": {}}
        ]})),
        other => Err(format!("unknown command {other}")),
    }
}
