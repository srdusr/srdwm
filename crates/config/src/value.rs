use mlua::Value;

/// A scalar (or short list of scalars) read from `srd.set(key, value)`.
/// Deliberately simpler than the legacy C++ `LuaConfigValue`, which also had
/// `Table`/`Function` variants that, per docs/PRIOR_ART.md, were never
/// actually exercised by the flat dotted-key config style the example
/// configs (and `docs/DEFAULTS.md`) actually use.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    String(String),
    Number(f64),
    Bool(bool),
    List(Vec<String>),
}

impl ConfigValue {
    pub fn from_lua(value: &Value) -> Option<Self> {
        match value {
            Value::String(s) => Some(ConfigValue::String(s.to_str().ok()?.to_string())),
            Value::Number(n) => Some(ConfigValue::Number(*n)),
            Value::Integer(i) => Some(ConfigValue::Number(*i as f64)),
            Value::Boolean(b) => Some(ConfigValue::Bool(*b)),
            Value::Table(t) => {
                let mut items = Vec::new();
                for pair in t.clone().sequence_values::<String>() {
                    items.push(pair.ok()?);
                }
                Some(ConfigValue::List(items))
            }
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            ConfigValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ConfigValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            ConfigValue::List(l) => Some(l),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    #[test]
    fn converts_string_number_bool_and_list() {
        let lua = Lua::new();
        assert_eq!(ConfigValue::from_lua(&lua.load("'hi'").eval().unwrap()), Some(ConfigValue::String("hi".into())));
        assert_eq!(ConfigValue::from_lua(&lua.load("42").eval().unwrap()), Some(ConfigValue::Number(42.0)));
        assert_eq!(ConfigValue::from_lua(&lua.load("true").eval().unwrap()), Some(ConfigValue::Bool(true)));
        assert_eq!(
            ConfigValue::from_lua(&lua.load("{'1','2','3'}").eval().unwrap()),
            Some(ConfigValue::List(vec!["1".into(), "2".into(), "3".into()]))
        );
    }
}
