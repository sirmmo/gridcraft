//! The plugin host against a real plugin process (`gridcraft-example-plugin`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use gridcraft_core::{CellError, Value};
use serde_json::{Value as Json, json};

/// The example plugin's names are fixed, so the tests take turns.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Writes the example plugin's manifest into a fresh folder and loads it.
fn load_example(tag: &str) -> (Arc<gridcraft_plugin::Plugin>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("gridcraft-plugin-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_gridcraft-example-plugin")).arg("--manifest").output().unwrap();
    std::fs::write(dir.join("plugin.json"), out.stdout).unwrap();
    (gridcraft_plugin::load(&dir).unwrap(), dir)
}

fn call(name: &str, args: &[Value]) -> Value {
    gridcraft_calc::external::get(name).unwrap_or_else(|| panic!("{name} not registered")).call(args)
}

#[test]
fn functions_answer_through_the_process() {
    let _g = serial();
    let (p, dir) = load_example("functions");
    assert_eq!(p.status(), "ready");
    assert_eq!(call("example.hello", &[Value::Text("world".into())]), Value::Text("Hello, world!".into()));
    assert_eq!(p.status(), "running");
    let range = gridcraft_core::Array::new(2, 1, vec![Value::number(1.0), Value::number(2.0)]).unwrap();
    assert_eq!(call("EXAMPLE.SUMSQ", &[Value::from(range), Value::number(3.0)]), Value::number(14.0));
    let Value::Array(a) = call("EXAMPLE.COUNTUP", &[Value::number(3.0)]) else { panic!() };
    assert_eq!((a.rows, a.cols), (3, 1));
    assert_eq!(call("EXAMPLE.COUNTUP", &[Value::number(0.0)]), Value::Error(CellError::Num));
    // Non-volatile results are cached; volatile ones call every time.
    let n1 = call("EXAMPLE.CALLS", &[]);
    let n2 = call("EXAMPLE.CALLS", &[]);
    assert_ne!(n1, n2);
    // Through a formula.
    let wb = gridcraft_model::Workbook::new();
    let v = gridcraft_calc::evaluate(&wb, 0, gridcraft_core::CellRef::new(0, 0), "=EXAMPLE.HELLO(\"you\")&\".\"");
    assert_eq!(v, Value::Text("Hello, you!.".into()));
    assert!(gridcraft_plugin::list().iter().any(|q| q.manifest.id == "org.gridcraft.example"));
    assert!(gridcraft_plugin::unload("org.gridcraft.example"));
    assert!(gridcraft_calc::external::get("EXAMPLE.HELLO").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn hangs_and_crashes_cost_an_error_value_and_restart_the_process() {
    let _g = serial();
    let (p, dir) = load_example("failures");
    // Longer than the declared 300 ms: #VALUE!, and the process is restarted on the next call.
    assert_eq!(call("EXAMPLE.SLEEP", &[Value::number(2_000.0)]), Value::Error(CellError::Value));
    assert_eq!(p.status(), "ready");
    assert_eq!(call("EXAMPLE.SLEEP", &[Value::number(1.0)]), Value::number(1.0));
    // A crash is the second failure; the third stops the plugin for good.
    assert_eq!(call("EXAMPLE.CRASH", &[]), Value::Error(CellError::Value));
    assert_eq!(call("EXAMPLE.HELLO", &[Value::Text("again".into())]), Value::Text("Hello, again!".into()));
    assert_eq!(call("EXAMPLE.CRASH", &[]), Value::Error(CellError::Value));
    assert!(p.status().starts_with("failed"), "{}", p.status());
    assert_eq!(call("EXAMPLE.HELLO", &[Value::Text("x".into())]), Value::Error(CellError::Value));
    // Reloading starts afresh.
    let (p, dir2) = load_example("failures2");
    assert_eq!(p.status(), "ready");
    assert_eq!(call("EXAMPLE.HELLO", &[Value::Text("x".into())]), Value::Text("Hello, x!".into()));
    gridcraft_plugin::unload("org.gridcraft.example");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}

#[test]
fn commands_reply_with_engine_commands() {
    let _g = serial();
    let (_p, dir) = load_example("commands");
    let ctx = json!({"active": "B2", "selection": "B2:B3", "values": [[1], [2]]});
    let r = gridcraft_plugin::run_command("org.gridcraft.example", "countUp", json!({"count": 2}), ctx.clone()).unwrap();
    assert_eq!(r.commands, vec![("range.setValues".to_string(), json!({"range": "B2", "values": [[1], [2]]}))]);
    assert_eq!(r.message.as_deref(), Some("Wrote 1 to 2"));
    let r = gridcraft_plugin::run_command("org.gridcraft.example", "reverse", Json::Null, ctx.clone()).unwrap();
    assert_eq!(r.commands[0].1["values"], json!([[2], [1]]));
    assert!(gridcraft_plugin::run_command("org.gridcraft.example", "nope", Json::Null, ctx.clone()).is_err());
    assert!(gridcraft_plugin::run_command("no.plugin", "countUp", Json::Null, ctx).is_err());
    gridcraft_plugin::unload("org.gridcraft.example");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_program_is_an_error_value() {
    let _g = serial();
    let m = gridcraft_plugin::Manifest::parse(
        r#"{"id": "t.missing", "name": "Missing", "run": {"command": "./no-such-program"}, "functions": [{"name": "MISSING.F"}]}"#,
    )
    .unwrap();
    gridcraft_plugin::install(m, std::env::temp_dir()).unwrap();
    assert_eq!(call("MISSING.F", &[]), Value::Error(CellError::Value));
    gridcraft_plugin::unload("t.missing");
}

#[test]
fn discover_finds_plugin_folders() {
    let root = std::env::temp_dir().join(format!("gridcraft-plugin-discover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for d in ["b", "a", "not-a-plugin"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(root.join("a/plugin.json"), "{}").unwrap();
    std::fs::write(root.join("b/plugin.json"), "{}").unwrap();
    let found = gridcraft_plugin::discover(std::slice::from_ref(&root));
    assert_eq!(found, vec![root.join("a"), root.join("b")]);
    assert_eq!(gridcraft_plugin::discover(&[root.join("a")]), vec![root.join("a")]);
    let _ = std::fs::remove_dir_all(root);
}
