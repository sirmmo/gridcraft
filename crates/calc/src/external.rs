//! External (add-in) functions: formulas call them by name when no built-in function or
//! LAMBDA name matches. Plugins register them here (`crates/plugin`); the registry is
//! process-wide, like Excel's add-ins, so every evaluation path (cells, conditional formats,
//! validation, charts, the CLI's `eval`) sees them.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use gridcraft_core::Value;

/// A function provided from outside the built-in library.
pub trait ExternalFunction: Send + Sync {
    /// Calls the function with its evaluated arguments (ranges arrive as arrays). Never panics:
    /// a failure is an error value.
    fn call(&self, args: &[Value]) -> Value;
    /// Recalculated on every recalculation, like `RAND`.
    fn volatile(&self) -> bool {
        false
    }
    fn min_args(&self) -> usize {
        0
    }
    /// Excel allows at most 255 arguments.
    fn max_args(&self) -> usize {
        255
    }
}

type Registry = RwLock<HashMap<String, Arc<dyn ExternalFunction>>>;

fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(|| RwLock::new(HashMap::new()))
}

fn key(name: &str) -> String {
    name.trim().to_ascii_uppercase()
}

/// Registers (or replaces) the function called `name` (case-insensitive).
pub fn register(name: &str, f: Arc<dyn ExternalFunction>) {
    let mut reg = match registry().write() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    reg.insert(key(name), f);
}

/// Removes the function called `name`; true when it was registered.
pub fn unregister(name: &str) -> bool {
    let mut reg = match registry().write() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    reg.remove(&key(name)).is_some()
}

/// The function called `name`, if one is registered.
pub fn get(name: &str) -> Option<Arc<dyn ExternalFunction>> {
    let reg = match registry().read() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    reg.get(&key(name)).cloned()
}

/// Names of the registered functions, sorted.
pub fn names() -> Vec<String> {
    let reg = match registry().read() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut v: Vec<String> = reg.keys().cloned().collect();
    v.sort();
    v
}
