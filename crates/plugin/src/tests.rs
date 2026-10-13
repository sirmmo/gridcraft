use gridcraft_core::{CellError, Value};
use serde_json::json;

use crate::values::{from_json, to_json};
use crate::{Manifest, parse_reply};

fn manifest(functions: serde_json::Value) -> String {
    json!({"id": "t.plugin", "name": "T", "run": {"command": "true"}, "functions": functions}).to_string()
}

#[test]
fn manifests_are_validated() {
    let m = Manifest::parse(&manifest(json!([{"name": "t.one", "maxArgs": 2}]))).unwrap();
    assert_eq!(m.functions[0].name, "T.ONE");
    assert_eq!((m.functions[0].min_args, m.functions[0].max_args), (0, 2));
    for bad in [
        json!([{"name": "1BAD"}]),
        json!([{"name": "BAD NAME"}]),
        json!([{"name": "T.X"}, {"name": "t.x"}]),
        json!([{"name": "T.X", "minArgs": 3, "maxArgs": 2}]),
        json!([{"name": "T.X", "maxArgs": 300}]),
    ] {
        assert!(Manifest::parse(&manifest(bad.clone())).is_err(), "{bad}");
    }
    assert!(Manifest::parse(r#"{"id": "bad id", "name": "x", "run": {"command": "x"}}"#).is_err());
    assert!(Manifest::parse(r#"{"id": "x", "name": "x", "run": {"command": ""}}"#).is_err());
    assert!(Manifest::parse("not json").is_err());
}

#[test]
fn ui_declarations_must_name_the_plugins_commands() {
    let with_ui = |ui: serde_json::Value| {
        json!({"id": "t.ui", "name": "T", "run": {"command": "true"},
            "commands": [{"id": "go", "label": "Go"}], "ui": ui})
        .to_string()
    };
    let m = Manifest::parse(&with_ui(json!({
        "ribbon": [{"tab": "Data", "group": "Mine", "buttons": [{"command": "go", "size": "small", "params": {"n": 1}}]}],
        "menus": [{"command": "go"}, {"menu": "sheetTab", "command": "go", "label": "Go here"}]
    })))
    .unwrap();
    assert_eq!(m.ui.ribbon[0].buttons[0].size, crate::ButtonSize::Small);
    assert_eq!(m.ui.menus[0].menu, crate::MenuKind::Cell);
    assert_eq!(m.ui.menus[1].menu, crate::MenuKind::SheetTab);
    assert_eq!(m.command("go").map(|c| c.label.as_str()), Some("Go"));
    for bad in [
        json!({"ribbon": [{"tab": "Data", "group": "G", "buttons": [{"command": "nope"}]}]}),
        json!({"ribbon": [{"tab": "Data", "group": "G", "buttons": []}]}),
        json!({"ribbon": [{"tab": " ", "group": "G", "buttons": [{"command": "go"}]}]}),
        json!({"ribbon": [{"tab": "Data", "group": "G", "buttons": [{"command": "go", "size": "huge"}]}]}),
        json!({"menus": [{"command": "nope"}]}),
        json!({"menus": [{"menu": "statusBar", "command": "go"}]}),
        json!({"menus": [{"command": "go", "label": "x".repeat(65)}]}),
    ] {
        assert!(Manifest::parse(&with_ui(bad.clone())).is_err(), "{bad}");
    }
}

#[test]
fn built_in_function_names_are_refused() {
    let m = Manifest::parse(&manifest(json!([{"name": "SUM"}]))).unwrap();
    let e = crate::install(m, std::env::temp_dir()).err().unwrap();
    assert!(e.contains("built-in"), "{e}");
}

#[test]
fn values_round_trip() {
    let arr = from_json(&json!([[1, "a"], [true, {"error": "#N/A"}]]));
    let Value::Array(a) = &arr else { panic!("{arr:?}") };
    assert_eq!((a.rows, a.cols), (2, 2));
    assert_eq!(a.get(1, 1), Some(&Value::Error(CellError::NA)));
    assert_eq!(to_json(&arr), json!([[1.0, "a"], [true, {"error": "#N/A"}]]));
    assert_eq!(from_json(&json!(null)), Value::Empty);
    assert_eq!(from_json(&json!({"error": "#nope"})), Value::Error(CellError::Value));
    assert_eq!(from_json(&json!({"x": 1})), Value::Error(CellError::Value));
    assert_eq!(from_json(&json!([])), Value::Error(CellError::Calc));
    assert_eq!(from_json(&json!([[]])), Value::Error(CellError::Calc));
    assert_eq!(from_json(&json!([[[1]]])), from_json(&json!([[{"error": "#VALUE!"}]])));
    // A flat list is a row; ragged rows are padded with #N/A.
    let Value::Array(row) = from_json(&json!([1, 2, 3])) else { panic!() };
    assert_eq!((row.rows, row.cols), (1, 3));
    let Value::Array(r) = from_json(&json!([[1], [2, 3]])) else { panic!() };
    assert_eq!(r.get(0, 1), Some(&Value::Error(CellError::NA)));
    // Text is capped at Excel's 32,767 characters.
    let Value::Text(t) = from_json(&json!("é".repeat(40_000))) else { panic!() };
    assert_eq!(t.chars().count(), 32_767);
}

#[test]
fn command_replies_are_checked() {
    let r = parse_reply(json!({"commands": [{"command": "cell.set", "params": {"cell": "A1"}}], "message": "hi"})).unwrap();
    assert_eq!(r.commands.len(), 1);
    assert_eq!(r.message.as_deref(), Some("hi"));
    assert!(parse_reply(json!({"commands": {}})).is_err());
    assert!(parse_reply(json!({"commands": [{"params": {}}]})).is_err());
    assert!(parse_reply(json!(null)).unwrap().commands.is_empty());
}
