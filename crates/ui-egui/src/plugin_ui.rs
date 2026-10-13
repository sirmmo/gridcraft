//! What plugins add to the app: ribbon groups (on a built-in tab or a tab of their own) and
//! right-click menu items, declared in their manifest's `ui` section. Each runs one of the
//! plugin's commands through `plugins.run`.

use egui::Ui;
use gridcraft_engine::plugin::{self, ButtonSize, MenuKind};
use serde_json::{Value as Json, json};

use crate::SheetApp;
use crate::icons::Icon;
use crate::widgets::{big_button, small_button};

/// One button or menu item.
pub struct Item {
    pub plugin: String,
    pub command: String,
    pub label: String,
    pub tooltip: String,
    pub icon: Icon,
    pub small: bool,
    pub params: Json,
}

/// The tabs plugins add (names not among the built-in tabs), in load order.
pub fn extra_tabs() -> Vec<String> {
    let mut tabs: Vec<String> = Vec::new();
    for p in plugin::list() {
        for g in &p.manifest.ui.ribbon {
            if !plugin::RIBBON_TABS.contains(&g.tab.as_str()) && !tabs.contains(&g.tab) {
                tabs.push(g.tab.clone());
            }
        }
    }
    tabs
}

/// The plugin groups on `tab`: (group name, buttons).
pub fn groups(tab: &str) -> Vec<(String, Vec<Item>)> {
    let mut out = Vec::new();
    for p in plugin::list() {
        let m = &p.manifest;
        for g in m.ui.ribbon.iter().filter(|g| g.tab == tab) {
            let buttons = g
                .buttons
                .iter()
                .map(|b| {
                    let decl = m.command(&b.command);
                    Item {
                        plugin: m.id.clone(),
                        command: b.command.clone(),
                        label: b.label.clone().or_else(|| decl.map(|c| c.label.clone())).unwrap_or_else(|| b.command.clone()),
                        tooltip: b.tooltip.clone().or_else(|| decl.map(|c| c.description.clone())).unwrap_or_default(),
                        icon: b.icon.as_deref().and_then(icon_named).unwrap_or(Icon::Script),
                        small: b.size == ButtonSize::Small,
                        params: b.params.clone(),
                    }
                })
                .collect();
            out.push((g.group.clone(), buttons));
        }
    }
    out
}

/// The plugin items for a right-click menu.
pub fn menu_items(kind: MenuKind) -> Vec<Item> {
    let mut out = Vec::new();
    for p in plugin::list() {
        let m = &p.manifest;
        for item in m.ui.menus.iter().filter(|i| i.menu == kind) {
            let decl = m.command(&item.command);
            out.push(Item {
                plugin: m.id.clone(),
                command: item.command.clone(),
                label: item.label.clone().or_else(|| decl.map(|c| c.label.clone())).unwrap_or_else(|| item.command.clone()),
                tooltip: decl.map(|c| c.description.clone()).unwrap_or_default(),
                icon: Icon::Script,
                small: true,
                params: item.params.clone(),
            });
        }
    }
    out
}

/// Runs an item's command; `extra` fields (e.g. the right-clicked sheet) are added to its params.
pub fn run(app: &mut SheetApp, item: &Item, extra: Json) {
    let mut params = if item.params.is_object() { item.params.clone() } else { json!({}) };
    if let (Some(p), Json::Object(e)) = (params.as_object_mut(), extra) {
        for (k, v) in e {
            p.entry(k).or_insert(v);
        }
    }
    app.run_or_alert("plugins.run", json!({"plugin": item.plugin, "command": item.command, "params": params}));
}

/// Draws the plugin groups on `tab`, each after a separator; small buttons stack three high.
pub fn ribbon_groups(app: &mut SheetApp, ui: &mut Ui, tab: &str, separate_first: bool) {
    for (i, (_group, buttons)) in groups(tab).into_iter().enumerate() {
        if separate_first || i > 0 {
            crate::ribbon::sep(ui);
        }
        let mut k = 0;
        while let Some(b) = buttons.get(k) {
            if !b.small {
                if labelled(big_button(ui, b.icon, &b.label, &b.tooltip, false), &b.label).clicked() {
                    run(app, b, json!({}));
                }
                k += 1;
                continue;
            }
            let column: Vec<&Item> = buttons.iter().skip(k).take_while(|b| b.small).take(3).collect();
            k += column.len();
            ui.vertical(|ui| {
                for b in column {
                    if labelled(small_button(ui, b.icon, &b.label, &b.tooltip, false), &b.label).clicked() {
                        run(app, b, json!({}));
                    }
                }
            });
        }
    }
}

/// Names the button for screen readers (and tests).
fn labelled(resp: egui::Response, label: &str) -> egui::Response {
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    resp
}

/// The icon a manifest names (`"icon": "chart"`); case and `-`/`_` don't matter.
pub fn icon_named(name: &str) -> Option<Icon> {
    let key: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    Some(match key.as_str() {
        "script" | "plugin" => Icon::Script,
        "paste" => Icon::Paste,
        "copy" => Icon::Copy,
        "cut" => Icon::Cut,
        "insert" => Icon::Insert,
        "delete" => Icon::Delete,
        "format" => Icon::Format,
        "sum" => Icon::Sum,
        "function" => Icon::Function,
        "fx" => Icon::Fx,
        "calc" | "calculate" | "refresh" => Icon::Calc,
        "table" => Icon::Table,
        "pivottable" => Icon::PivotTable,
        "chart" => Icon::Chart,
        "chartbar" => Icon::ChartBar,
        "chartline" => Icon::ChartLine,
        "chartpie" => Icon::ChartPie,
        "chartscatter" => Icon::ChartScatter,
        "sparkline" => Icon::Sparkline,
        "picture" | "image" => Icon::Picture,
        "link" => Icon::Link,
        "comment" => Icon::Comment,
        "note" => Icon::Note,
        "filter" => Icon::Filter,
        "sort" | "sortasc" => Icon::SortAsc,
        "sortdesc" => Icon::SortDesc,
        "find" => Icon::Find,
        "search" => Icon::Search,
        "lock" => Icon::Lock,
        "settings" => Icon::Settings,
        "share" => Icon::Share,
        "sheet" => Icon::Sheet,
        "book" | "workbook" => Icon::Book,
        "folder" | "open" => Icon::OpenFolder,
        "save" => Icon::Save,
        "print" => Icon::Print,
        "calendar" | "date" => Icon::Calendar,
        "clock" | "time" => Icon::Clock,
        "info" => Icon::Info,
        "check" => Icon::Check,
        "plus" | "add" => Icon::Plus,
        "minus" => Icon::Minus,
        "pen" => Icon::Pen,
        "currency" | "money" => Icon::Currency,
        "percent" => Icon::Percent,
        "validation" => Icon::Validation,
        "name" => Icon::Name,
        "zoom" => Icon::Zoom,
        "more" => Icon::More,
        _ => return None,
    })
}
