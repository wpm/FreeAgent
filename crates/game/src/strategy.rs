//! What every game shares in its strategies: the prompted policy's
//! parameters, which a `[strategies.<name>]` table that asks a model holds.

use serde::{Deserialize, Serialize};

/// The prompted policy's parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PromptedTable")]
pub struct Prompted {
    /// The system prompt template.
    pub system: String,
    /// The name of the model asked. The file may leave it out when it
    /// defines exactly one, which the game's configuration fills in.
    pub model: String,
}

/// The prompted policy's parameters, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptedTable {
    system: String,
    model: Option<String>,
}

impl TryFrom<PromptedTable> for Prompted {
    type Error = &'static str;

    /// The parameters, with a model left out as no name yet. A model
    /// named by the empty name is an error.
    fn try_from(PromptedTable { system, model }: PromptedTable) -> Result<Self, Self::Error> {
        match model {
            Some(model) if model.is_empty() => Err("model is empty; name a model or leave it out"),
            model => Ok(Prompted {
                system,
                model: model.unwrap_or_default(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The parameters a `[strategies.<name>]` table of `table` parses into.
    fn parsed(table: &str) -> Result<Prompted, toml::de::Error> {
        toml::from_str(table)
    }

    #[test]
    fn a_prompted_strategy_parses_and_is_written_with_its_parameters() {
        let prompted = parsed("system = \"Hi.\"\nmodel = \"qwen\"").unwrap();
        assert_eq!(
            prompted,
            Prompted {
                system: "Hi.".to_string(),
                model: "qwen".to_string(),
            }
        );
        assert_eq!(
            serde_json::to_value(&prompted).unwrap(),
            json!({"system": "Hi.", "model": "qwen"})
        );
        // A model left out is no name yet.
        assert_eq!(parsed("system = \"Hi.\"").unwrap().model, "");
    }

    #[test]
    fn a_prompted_strategy_takes_its_parameters_and_only_those() {
        for table in [
            "system = \"Hi.\"\ntemperature = 0.5\n",
            "model = \"qwen\"\n",
        ] {
            assert!(parsed(table).is_err(), "{table}");
        }
        let error = parsed("system = \"Hi.\"\nmodel = \"\"").unwrap_err();
        assert!(error.to_string().contains("model is empty"), "{error}");
    }
}
