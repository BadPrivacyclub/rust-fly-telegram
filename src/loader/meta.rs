//! Module metadata declared in Lua: `M.help` (documentation) and `M.config` (settings schema).
//!
//! ```lua
//! M.help = {
//!     category = "messaging",
//!     description = "Saved notes",
//!     commands = {
//!         note = { args = "set <text> | get | clear", desc = "Manage a saved note" },
//!         notes = "List notes",            -- shorthand: description only
//!     },
//! }
//!
//! M.config = {
//!     { key = "limit", type = "number", default = 10, description = "Max items" },
//!     { key = "mode", type = "select", options = { "fast", "safe" }, default = "safe" },
//!     { key = "api_key", type = "string", secret = true },
//! }
//! ```

use mlua::{LuaSerdeExt, Table, Value as LuaValue};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize)]
pub struct CommandHelp {
    pub name: String,
    pub args: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ModuleHelp {
    pub category: String,
    pub description: String,
    pub commands: Vec<CommandHelp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    String,
    Text,
    Number,
    Bool,
    Select,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfigField {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: FieldKind,
    pub default: Value,
    pub description: String,
    pub options: Vec<String>,
    pub secret: bool,
}

pub fn parse_help(lua: &mlua::Lua, table: &Table, commands: &[String]) -> ModuleHelp {
    let mut help = ModuleHelp::default();
    let declared = table.get::<Table>("help").ok();
    let mut documented = Vec::new();

    if let Some(declared) = &declared {
        help.category = declared.get::<String>("category").unwrap_or_default();
        help.description = declared.get::<String>("description").unwrap_or_default();
        if let Ok(entries) = declared.get::<Table>("commands") {
            for pair in entries.pairs::<String, LuaValue>() {
                let Ok((name, value)) = pair else {
                    continue;
                };
                let (args, description) = match value {
                    LuaValue::String(text) => (String::new(), text.to_string_lossy()),
                    LuaValue::Table(entry) => (
                        entry.get::<String>("args").unwrap_or_default(),
                        entry
                            .get::<String>("desc")
                            .or_else(|_| entry.get::<String>("description"))
                            .unwrap_or_default(),
                    ),
                    _ => continue,
                };
                documented.push(CommandHelp {
                    name: name.to_lowercase(),
                    args,
                    description,
                });
            }
        }
    }

    // Legacy modules only list handlers; keep every command visible.
    for command in commands {
        if !documented.iter().any(|entry| &entry.name == command) {
            documented.push(CommandHelp {
                name: command.clone(),
                ..CommandHelp::default()
            });
        }
    }
    documented.retain(|entry| commands.contains(&entry.name));
    documented.sort_by(|a, b| a.name.cmp(&b.name));
    help.commands = documented;
    let _ = lua;
    help
}

pub fn parse_config(lua: &mlua::Lua, table: &Table) -> Vec<ConfigField> {
    let Ok(config) = table.get::<Table>("config") else {
        return Vec::new();
    };
    let mut fields = Vec::new();

    let mut push = |key: String, entry: &Table| {
        if key.is_empty() || fields.iter().any(|field: &ConfigField| field.key == key) {
            return;
        }
        let kind = match entry
            .get::<String>("type")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "number" | "int" | "integer" | "float" => FieldKind::Number,
            "bool" | "boolean" | "toggle" => FieldKind::Bool,
            "select" | "choice" | "enum" => FieldKind::Select,
            "text" | "multiline" => FieldKind::Text,
            _ => FieldKind::String,
        };
        let options = entry.get::<Vec<String>>("options").unwrap_or_default();
        let default = entry
            .get::<LuaValue>("default")
            .ok()
            .and_then(|value| lua.from_value::<Value>(value).ok())
            .unwrap_or(Value::Null);
        fields.push(ConfigField {
            key,
            kind: if kind == FieldKind::String && !options.is_empty() {
                FieldKind::Select
            } else {
                kind
            },
            default,
            description: entry
                .get::<String>("description")
                .or_else(|_| entry.get::<String>("desc"))
                .unwrap_or_default(),
            options,
            secret: entry.get::<bool>("secret").unwrap_or(false),
        });
    };

    // Array form keeps declaration order.
    for entry in config.clone().sequence_values::<Table>().flatten() {
        let key = entry.get::<String>("key").unwrap_or_default();
        push(key, &entry);
    }
    // Map form: { key = { ... } }, sorted for a stable order.
    let mut mapped = config
        .pairs::<LuaValue, Table>()
        .flatten()
        .filter_map(|(key, entry)| match key {
            LuaValue::String(key) => Some((key.to_string_lossy(), entry)),
            _ => None,
        })
        .collect::<Vec<_>>();
    mapped.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, entry) in mapped {
        push(key, &entry);
    }
    fields
}

/// Converts operator input (from Telegram text, the bot, or the panel) to the field type.
pub fn coerce(field: &ConfigField, raw: &Value) -> anyhow::Result<Value> {
    let text = match raw {
        Value::String(text) => Some(text.trim().to_string()),
        _ => None,
    };
    match field.kind {
        FieldKind::Bool => match raw {
            Value::Bool(value) => Ok(Value::Bool(*value)),
            _ => match text.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("on" | "true" | "1" | "yes" | "y" | "вкл" | "да") => {
                    Ok(Value::Bool(true))
                }
                Some("off" | "false" | "0" | "no" | "n" | "выкл" | "нет") => {
                    Ok(Value::Bool(false))
                }
                _ => anyhow::bail!("'{}' expects on/off", field.key),
            },
        },
        FieldKind::Number => {
            let number = match raw {
                Value::Number(number) => number.as_f64(),
                _ => text.as_deref().and_then(|text| text.parse::<f64>().ok()),
            }
            .ok_or_else(|| anyhow::anyhow!("'{}' expects a number", field.key))?;
            Ok(if number.fract() == 0.0 && number.abs() < 9e15 {
                Value::from(number as i64)
            } else {
                serde_json::Number::from_f64(number)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            })
        }
        FieldKind::Select => {
            let value = text.unwrap_or_else(|| raw.to_string());
            if !field.options.is_empty() && !field.options.contains(&value) {
                anyhow::bail!(
                    "'{}' must be one of: {}",
                    field.key,
                    field.options.join(", ")
                );
            }
            Ok(Value::String(value))
        }
        FieldKind::String | FieldKind::Text => match raw {
            Value::String(text) => Ok(Value::String(text.clone())),
            Value::Null => Ok(Value::Null),
            other => Ok(Value::String(other.to_string())),
        },
    }
}

pub fn display_value(field: &ConfigField, value: &Value) -> String {
    if field.secret {
        return match value {
            Value::Null => "—".to_string(),
            Value::String(text) if text.is_empty() => "—".to_string(),
            _ => "••••••".to_string(),
        };
    }
    match value {
        Value::Null => "—".to_string(),
        Value::String(text) => text.clone(),
        Value::Bool(true) => "on".to_string(),
        Value::Bool(false) => "off".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(source: &str) -> (mlua::Lua, Table) {
        let lua = mlua::Lua::new();
        let table = lua.load(source).eval::<Table>().unwrap();
        (lua, table)
    }

    #[test]
    fn parses_help_and_fills_missing_commands() {
        let (lua, table) = eval(
            r#"return { help = { category = "messaging", description = "Notes",
                commands = { note = { args = "set <text>", desc = "Save" }, ghost = "gone" } } }"#,
        );
        let help = parse_help(&lua, &table, &["note".into(), "notes".into()]);
        assert_eq!(help.category, "messaging");
        assert_eq!(help.commands.len(), 2);
        assert_eq!(help.commands[0].name, "note");
        assert_eq!(help.commands[0].args, "set <text>");
        assert_eq!(help.commands[1].name, "notes");
    }

    #[test]
    fn parses_config_in_both_forms() {
        let (lua, table) = eval(
            r#"return { config = {
                { key = "b", type = "bool", default = true },
                { key = "a", type = "select", options = { "x", "y" }, default = "x" },
            } }"#,
        );
        let fields = parse_config(&lua, &table);
        assert_eq!(
            fields.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        assert_eq!(fields[0].default, Value::Bool(true));

        let (lua, table) =
            eval(r#"return { config = { limit = { type = "number", default = 5 } } }"#);
        let fields = parse_config(&lua, &table);
        assert_eq!(fields[0].kind, FieldKind::Number);
    }

    #[test]
    fn coerces_values() {
        let field = |kind, options: &[&str]| ConfigField {
            key: "k".into(),
            kind,
            default: Value::Null,
            description: String::new(),
            options: options.iter().map(|s| s.to_string()).collect(),
            secret: false,
        };
        assert_eq!(
            coerce(&field(FieldKind::Bool, &[]), &Value::String("on".into())).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            coerce(&field(FieldKind::Number, &[]), &Value::String("12".into())).unwrap(),
            Value::from(12)
        );
        assert!(coerce(&field(FieldKind::Number, &[]), &Value::String("x".into())).is_err());
        assert!(coerce(
            &field(FieldKind::Select, &["a"]),
            &Value::String("b".into())
        )
        .is_err());
    }
}
