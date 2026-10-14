//! A configuration file as the desktop app's settings screen edits it:
//! a TOML document whose comments, order and formatting survive every
//! edit. The screen draws the document's [`FormView`] and changes it one
//! [`Value`] at a time, at a TOML path. Nothing here checks the settings:
//! [`run::prepare`](crate::run::prepare) does, from [`Document::text`],
//! as the command line does from the file.

use crate::avalon;
use crate::config::Name;
use crate::model::key_variable;
use crate::model::spec::Spec;
use crate::werewolf;
use anyhow::{Context as _, anyhow};
use serde::Deserialize;
use serde::de::value::StrDeserializer;
use social_deduction_stage::form::{
    Field, FormView, KeyNeed, ModelView, RoleView, StrategyView, Value,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use toml_edit::{DocumentMut, Item, Table, TableLike};

/// Werewolf's `[phases]` keys, in the order the phases come.
const WEREWOLF_PHASES: [&str; 3] = ["night_limit", "day_limit", "ratify_limit"];

/// Avalon's `[phases]` keys, in the order the phases come.
const AVALON_PHASES: [&str; 4] = [
    "propose_limit",
    "vote_limit",
    "quest_limit",
    "assassinate_limit",
];

/// A configuration file being edited.
#[derive(Debug, Clone)]
pub struct Document(DocumentMut);

impl Document {
    /// The document `text` holds.
    ///
    /// # Errors
    ///
    /// When `text` is not TOML: the parser's message, with the line and
    /// column.
    pub fn parse(text: &str) -> Result<Self, String> {
        text.parse()
            .map(Document)
            .map_err(|error: toml_edit::TomlError| error.to_string())
    }

    /// The document's text, as a configuration file holds it.
    pub fn text(&self) -> String {
        self.0.to_string()
    }

    /// Replace the whole document with the one `text` holds.
    ///
    /// # Errors
    ///
    /// When `text` is not TOML, as [`Document::parse`] says, and the
    /// document is left as it was.
    pub fn replace(&mut self, text: &str) -> Result<(), String> {
        *self = Document::parse(text)?;
        Ok(())
    }

    /// Set the key at `path`, such as `["phases", "day_limit"]`, to
    /// `value`, adding the tables on the way that are missing. Everything
    /// else in the document stays as it was, the key's comments included.
    ///
    /// # Errors
    ///
    /// When `path` is empty, or passes through a value that is not a table.
    pub fn set(&mut self, path: &[String], value: Value) -> Result<(), String> {
        let (last, parents) = path.split_last().ok_or("an edit names no key")?;
        let mut table: &mut dyn TableLike = self.0.as_table_mut();
        for (depth, key) in parents.iter().enumerate() {
            table = table
                .entry(key)
                .or_insert_with(|| {
                    let mut table = Table::new();
                    table.set_implicit(true);
                    Item::Table(table)
                })
                .as_table_like_mut()
                .ok_or_else(|| format!("{} is not a table", path[..=depth].join(".")))?;
        }
        match value {
            Value::Remove => drop(table.remove(last)),
            Value::Text(text) => put(
                table,
                last,
                |old| old.as_str() == Some(&text),
                || multi_line(&text),
            ),
            Value::Integer(integer) => put(
                table,
                last,
                |old| old.as_integer() == Some(integer),
                || integer.into(),
            ),
            Value::Table => {
                if !table.contains_key(last) {
                    table.insert(last, Item::Table(Table::new()));
                }
            }
        }
        Ok(())
    }

    /// Every setting the document holds, for the form.
    pub fn view(&self) -> FormView {
        let root = self.0.as_item();
        let shown = |path: &[&str]| shown(at(root, path));
        let game = shown(&["game"]);
        let (counts, phases): (Vec<(String, usize)>, &[&str]) = match game
            .as_deref()
            .and_then(named)
        {
            Some(Name::Werewolf) => (keyed(werewolf::Table::default().counts()), &WEREWOLF_PHASES),
            Some(Name::Avalon) => (keyed(avalon::Table::default().counts()), &AVALON_PHASES),
            None => (Vec::new(), &[]),
        };
        let roles = counts
            .into_iter()
            .map(|(key, default)| {
                let role = at(root, &["roles", &key]);
                RoleView {
                    count: shown(&["roles", &key, "count"]),
                    seats: seats(role, default),
                    strategy: shown(&["roles", &key, "strategy"]),
                    key,
                }
            })
            .collect();
        let phases = phases
            .iter()
            .map(|key| Field {
                key: key.to_string(),
                value: shown(&["phases", key]),
            })
            .collect();
        let strategies = names(root, "strategies")
            .into_iter()
            .map(|name| StrategyView {
                policy: shown(&["strategies", &name, "policy"]),
                model: shown(&["strategies", &name, "model"]),
                system: shown(&["strategies", &name, "system"]),
                name,
            })
            .collect();
        let models: Vec<ModelView> = names(root, "models")
            .into_iter()
            .map(|name| ModelView {
                base_url: shown(&["models", &name, "base_url"]),
                id: shown(&["models", &name, "id"]),
                api_key_env: shown(&["models", &name, "api_key_env"]),
                context_window: shown(&["models", &name, "context_window"]),
                name,
            })
            .collect();
        let text = names(root, "text")
            .into_iter()
            .map(|key| Field {
                value: shown(&["text", &key]),
                key,
            })
            .collect();
        let mut keys = BTreeMap::<String, Vec<String>>::new();
        for model in &models {
            let base_url = model.base_url.as_deref().unwrap_or_default();
            if let Some(variable) = key_variable(base_url, model.api_key_env.as_deref()) {
                keys.entry(variable.to_string())
                    .or_default()
                    .push(model.name.clone());
            }
        }
        let keys = keys
            .into_iter()
            .map(|(variable, models)| KeyNeed { variable, models })
            .collect();
        FormView {
            strategy: shown(&["strategy"]),
            game,
            roles,
            phases,
            strategies,
            models,
            text,
            keys,
        }
    }

    /// The `[models.<name>]` table, parsed as a game parses it.
    ///
    /// # Errors
    ///
    /// When the document has no such model, or its table does not parse.
    pub fn model(&self, name: &str) -> anyhow::Result<Spec> {
        let table = at(self.0.as_item(), &["models", name])
            .and_then(Item::as_table_like)
            .ok_or_else(|| anyhow!("there is no model {name}"))?;
        let mut alone = DocumentMut::new();
        for (key, item) in table.iter() {
            alone.insert(key, item.clone());
        }
        toml::from_str(&alone.to_string()).with_context(|| format!("[models.{name}]"))
    }
}

/// The game `game` names, if it names one.
fn named(game: &str) -> Option<Name> {
    Name::deserialize(StrDeserializer::<serde::de::value::Error>::new(game)).ok()
}

/// Each role of `counts` by its key, `werewolf` or `merlin`, with its
/// count.
fn keyed<R: std::fmt::Display>(
    counts: impl IntoIterator<Item = (R, usize)>,
) -> Vec<(String, usize)> {
    counts
        .into_iter()
        .map(|(role, count)| (role.to_string().to_lowercase(), count))
        .collect()
}

/// The item at `path` under `item`, if there is one.
fn at<'a>(item: &'a Item, path: &[&str]) -> Option<&'a Item> {
    path.iter()
        .try_fold(item, |item, key| item.as_table_like()?.get(key))
}

/// The text of the value `item` holds: a string's content, or any other
/// value as TOML writes it. A table holds no value.
fn shown(item: Option<&Item>) -> Option<String> {
    let value = item?.as_value()?;
    Some(match value.as_str() {
        Some(text) => text.to_string(),
        None => value.clone().decorated("", "").to_string(),
    })
}

/// The keys of the table `key` under `root`, in the document's order.
fn names(root: &Item, key: &str) -> Vec<String> {
    at(root, &[key])
        .and_then(Item::as_table_like)
        .map(|table| table.iter().map(|(name, _)| name.to_string()).collect())
        .unwrap_or_default()
}

/// How many seats the `[roles.*]` table `role` gives its role, as a
/// game's configuration allots them: its count, or, with no count, what
/// its `strategies` allot, with a `strategy` filling the rest up to the
/// game's `default`, or else the default.
fn seats(role: Option<&Item>, default: usize) -> usize {
    let integer = |item: &Item| item.as_integer().and_then(|n| usize::try_from(n).ok());
    let Some(role) = role.and_then(Item::as_table_like) else {
        return default;
    };
    if let Some(count) = role.get("count").and_then(integer) {
        return count;
    }
    let Some(strategies) = role.get("strategies").and_then(Item::as_table_like) else {
        return default;
    };
    let allotted: usize = strategies.iter().filter_map(|(_, n)| integer(n)).sum();
    match role.get("strategy") {
        Some(_) => allotted.max(default),
        None => allotted,
    }
}

/// Set `key` in `table` to the value `new` makes, unless what it holds
/// is the `same` already. A value replaced keeps its comments.
fn put(
    table: &mut dyn TableLike,
    key: &str,
    same: impl Fn(&toml_edit::Value) -> bool,
    new: impl FnOnce() -> toml_edit::Value,
) {
    match table.get_mut(key) {
        Some(Item::Value(old)) if same(old) => {}
        Some(Item::Value(old)) => {
            let decor = old.decor().clone();
            *old = new();
            *old.decor_mut() = decor;
        }
        _ => drop(table.insert(key, Item::Value(new()))),
    }
}

/// `text` as a TOML string: a multi-line one, its first line on a line
/// of its own, when it runs to more than one line.
fn multi_line(text: &str) -> toml_edit::Value {
    if !text.contains('\n') {
        return text.into();
    }
    let mut body = String::new();
    let mut quotes = 0;
    for c in text.chars() {
        quotes = if c == '"' { quotes + 1 } else { 0 };
        match c {
            // A third quote in a row would end the string.
            '"' if quotes == 3 => {
                body.push_str("\\\"");
                quotes = 0;
            }
            '\\' => body.push_str("\\\\"),
            '\n' | '\t' => body.push(c),
            c if c.is_control() => write!(body, "\\u{:04X}", u32::from(c)).unwrap(),
            c => body.push(c),
        }
    }
    format!("\"\"\"\n{body}\"\"\"")
        .parse()
        .expect("an escaped multi-line string parses")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::spec::Environment;
    use crate::run::prepare;
    use std::path::Path;

    /// The text of the example file `name` in `configs/`, with Unix line
    /// endings, as a Windows checkout may not give it.
    fn example(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("configs")
            .join(name);
        std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
    }

    /// The examples, by name.
    const EXAMPLES: [&str; 4] = [
        "werewolf/random.toml",
        "werewolf/prompted.toml",
        "avalon/random.toml",
        "avalon/prompted.toml",
    ];

    /// `path` as a document path.
    fn path(path: &[&str]) -> Vec<String> {
        path.iter().map(ToString::to_string).collect()
    }

    /// The document `text` holds, with `value` set at `at`.
    fn edited(text: &str, at: &[&str], value: Value) -> String {
        let mut document = Document::parse(text).unwrap();
        document.set(&path(at), value).unwrap();
        document.text()
    }

    /// Every edit that sets each value `view` shows to what it shows.
    fn same_edits(view: &FormView) -> Vec<(Vec<String>, Value)> {
        let text = |value: &Option<String>| value.clone().map_or(Value::Remove, Value::Text);
        let mut edits = vec![(path(&["strategy"]), text(&view.strategy))];
        for role in &view.roles {
            edits.push((
                path(&["roles", &role.key, "strategy"]),
                text(&role.strategy),
            ));
        }
        for phase in &view.phases {
            edits.push((path(&["phases", &phase.key]), text(&phase.value)));
        }
        for strategy in &view.strategies {
            for (key, value) in [
                ("policy", &strategy.policy),
                ("model", &strategy.model),
                ("system", &strategy.system),
            ] {
                edits.push((path(&["strategies", &strategy.name, key]), text(value)));
            }
        }
        for model in &view.models {
            for (key, value) in [("base_url", &model.base_url), ("id", &model.id)] {
                edits.push((path(&["models", &model.name, key]), text(value)));
            }
        }
        for block in &view.text {
            edits.push((path(&["text", &block.key]), text(&block.value)));
        }
        edits
    }

    #[test]
    fn every_example_loads_and_setting_what_its_view_shows_changes_nothing() {
        for name in EXAMPLES {
            let text = example(name);
            let mut document = Document::parse(&text).unwrap();
            assert_eq!(document.text(), text, "{name}");
            let view = document.view();
            for (at, value) in same_edits(&view) {
                document.set(&at, value).unwrap();
            }
            assert_eq!(document.text(), text, "{name}");
            assert_eq!(document.view(), view, "{name}");
        }
    }

    #[test]
    fn the_view_of_a_prompted_example_shows_its_settings() {
        let view = Document::parse(&example("werewolf/prompted.toml"))
            .unwrap()
            .view();
        assert_eq!(view.game.as_deref(), Some("werewolf"));
        assert_eq!(view.strategy.as_deref(), Some("everyone"));
        let roles: Vec<(&str, usize)> = view
            .roles
            .iter()
            .map(|role| (role.key.as_str(), role.seats))
            .collect();
        assert_eq!(
            roles,
            [("werewolf", 2), ("villager", 3), ("doctor", 1), ("seer", 1)]
        );
        assert_eq!(view.table_size(), 7);
        let phases: Vec<(&str, Option<&str>)> = view
            .phases
            .iter()
            .map(|field| (field.key.as_str(), field.value.as_deref()))
            .collect();
        assert_eq!(
            phases,
            [
                ("night_limit", None),
                ("day_limit", None),
                ("ratify_limit", Some("20s"))
            ]
        );
        assert_eq!(view.strategies.len(), 1);
        assert_eq!(view.strategies[0].name, "everyone");
        assert_eq!(view.strategies[0].policy.as_deref(), Some("prompted"));
        assert_eq!(view.strategies[0].model, None);
        assert!(
            view.strategies[0]
                .system
                .as_ref()
                .unwrap()
                .contains("{{ rules }}")
        );
        assert_eq!(
            view.models,
            [ModelView {
                name: "qwen".to_string(),
                base_url: Some("http://localhost:1234/v1".to_string()),
                id: Some("qwen2.5-7b-instruct-1m".to_string()),
                api_key_env: None,
                context_window: None,
            }]
        );
        assert_eq!(view.text.len(), 1);
        assert_eq!(view.text[0].key, "rules");
        // A local server needs no key.
        assert!(view.keys.is_empty());
        let avalon = Document::parse(&example("avalon/random.toml"))
            .unwrap()
            .view();
        let keys: Vec<&str> = avalon.roles.iter().map(|role| role.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "merlin", "percival", "servant", "assassin", "morgana", "mordred", "oberon",
                "minion"
            ]
        );
        assert_eq!(avalon.roles[0].strategy.as_deref(), Some("random"));
        assert_eq!(avalon.table_size(), 5);
        assert_eq!(avalon.phases.len(), 4);
    }

    #[test]
    fn a_document_naming_no_game_shows_no_roles_or_phases() {
        let view = Document::parse("strategy = \"random\"\n").unwrap().view();
        assert_eq!(view.game, None);
        assert!(view.roles.is_empty() && view.phases.is_empty());
        let view = Document::parse("game = \"chess\"\n").unwrap().view();
        assert_eq!(view.game.as_deref(), Some("chess"));
        assert!(view.roles.is_empty());
    }

    #[test]
    fn every_phase_the_form_shows_is_a_setting_of_its_game() {
        for (game, phases) in [
            ("werewolf", &WEREWOLF_PHASES[..]),
            ("avalon", &AVALON_PHASES[..]),
        ] {
            let mut text = format!("game = \"{game}\"\n[phases]\n");
            for phase in phases {
                writeln!(text, "{phase} = \"1m\"").unwrap();
            }
            crate::config::parse(&text).unwrap();
            let view = Document::parse(&text).unwrap().view();
            let shown: Vec<&str> = view.phases.iter().map(|f| f.key.as_str()).collect();
            assert_eq!(shown, phases);
        }
    }

    #[test]
    fn a_role_s_seats_are_its_count_its_allotted_strategies_or_the_game_s() {
        let view = |roles: &str| {
            Document::parse(&format!("game = \"werewolf\"\n{roles}"))
                .unwrap()
                .view()
        };
        let werewolves = |roles: &str| {
            let role = view(roles).roles.remove(0);
            (role.count, role.seats)
        };
        assert_eq!(werewolves(""), (None, 2));
        assert_eq!(
            werewolves("[roles.werewolf]\ncount = 3\n"),
            (Some("3".to_string()), 3)
        );
        assert_eq!(werewolves("[roles.werewolf]\ncount = \"many\"\n").1, 2);
        assert_eq!(
            werewolves("[roles.werewolf.strategies]\nbold = 1\nshy = 3\n").1,
            4
        );
        assert_eq!(
            werewolves(
                "[roles.werewolf]\nstrategy = \"shy\"\n[roles.werewolf.strategies]\nbold = 1\n"
            )
            .1,
            2
        );
        assert_eq!(
            werewolves(
                "[roles.werewolf]\nstrategy = \"shy\"\n[roles.werewolf.strategies]\nbold = 3\n"
            )
            .1,
            3
        );
        assert_eq!(werewolves("roles = { werewolf = 3 }\n").1, 2);
    }

    #[test]
    fn a_value_that_is_not_a_string_is_shown_as_toml_writes_it() {
        let view = Document::parse(
            "game = \"werewolf\"\n[roles.seer]\ncount = 2 # two seers\n\
             [models.big]\nbase_url = \"https://api.openai.com/v1\"\nid = \"gpt\"\ncontext_window = 32768\n",
        )
        .unwrap()
        .view();
        assert_eq!(view.roles[3].count.as_deref(), Some("2"));
        assert_eq!(view.models[0].context_window.as_deref(), Some("32768"));
        assert_eq!(
            view.keys,
            [KeyNeed {
                variable: "OPENAI_API_KEY".to_string(),
                models: vec!["big".to_string()],
            }]
        );
    }

    #[test]
    fn the_models_that_need_a_key_are_listed_under_it() {
        let view = Document::parse(
            "[models.a]\nbase_url = \"https://api.openai.com/v1\"\n\
             [models.b]\nbase_url = \"http://localhost:1234/v1\"\napi_key_env = \"MY_KEY\"\n\
             [models.c]\nbase_url = \"https://api.openai.com/v1\"\n[models.d]\n",
        )
        .unwrap()
        .view();
        let keys: Vec<(&str, Vec<&str>)> = view
            .keys
            .iter()
            .map(|need| {
                (
                    need.variable.as_str(),
                    need.models.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [("MY_KEY", vec!["b"]), ("OPENAI_API_KEY", vec!["a", "c"])]
        );
    }

    /// A commented configuration file.
    const COMMENTED: &str = "\
# The game.
game = \"werewolf\"

# How long each phase waits.
[phases]
# By day.
day_limit = \"45s\" # three quarters of a minute
ratify_limit = \"20s\"

# The seer.
[roles.seer]
count = 1 # just one
";

    #[test]
    fn setting_a_value_changes_only_the_value_and_keeps_every_comment() {
        assert_eq!(
            edited(
                COMMENTED,
                &["phases", "day_limit"],
                Value::Text("90s".into())
            ),
            COMMENTED.replace("\"45s\"", "\"90s\"")
        );
        assert_eq!(
            edited(COMMENTED, &["roles", "seer", "count"], Value::Integer(2)),
            COMMENTED.replace("count = 1", "count = 2")
        );
    }

    #[test]
    fn a_key_removed_takes_its_line_and_leaves_the_rest() {
        let removed = COMMENTED.replace("ratify_limit = \"20s\"\n", "");
        assert_eq!(
            edited(COMMENTED, &["phases", "ratify_limit"], Value::Remove),
            removed
        );
        assert_eq!(edited(COMMENTED, &["no", "such"], Value::Remove), COMMENTED);
    }

    #[test]
    fn a_key_or_a_table_that_is_missing_is_added_with_the_tables_on_its_way() {
        let added = edited(
            COMMENTED,
            &["roles", "werewolf", "count"],
            Value::Integer(3),
        );
        assert!(added.starts_with(COMMENTED), "{added}");
        assert!(added.ends_with("[roles.werewolf]\ncount = 3\n"), "{added}");
        let added = edited(COMMENTED, &["strategies", "bold"], Value::Table);
        assert!(added.ends_with("\n[strategies.bold]\n"), "{added}");
        assert!(!added.contains("[strategies]"), "{added}");
        // A table already there is left as it is.
        assert_eq!(edited(COMMENTED, &["phases"], Value::Table), COMMENTED);
        let added = edited(COMMENTED, &["strategy"], Value::Text("random".into()));
        assert!(added.contains("strategy = \"random\""), "{added}");
        crate::config::parse(&added).unwrap();
    }

    #[test]
    fn an_edit_naming_no_key_or_passing_through_a_value_is_refused() {
        let mut document = Document::parse(COMMENTED).unwrap();
        assert_eq!(
            document.set(&[], Value::Remove).unwrap_err(),
            "an edit names no key"
        );
        assert_eq!(
            document
                .set(&path(&["game", "x"]), Value::Integer(1))
                .unwrap_err(),
            "game is not a table"
        );
        assert_eq!(document.text(), COMMENTED);
    }

    #[test]
    fn text_of_several_lines_is_written_as_a_multi_line_string_that_reads_back_the_same() {
        let added = edited(
            "[text]\n",
            &["text", "rules"],
            Value::Text("Play well.\nWin.\n".into()),
        );
        assert_eq!(added, "[text]\nrules = \"\"\"\nPlay well.\nWin.\n\"\"\"\n");
        for text in [
            "a\nb",
            "say \"\"\"hi\"\"\"\n",
            "ends in quotes\n\"\"",
            "back\\slash\n\ttab\n",
            "bell\u{7}\r\n",
            "\"\"\"\"\"\"\n",
        ] {
            let value = multi_line(text);
            assert_eq!(value.as_str(), Some(text), "{value}");
        }
    }

    #[test]
    fn replacing_the_document_with_text_that_is_not_toml_leaves_it_as_it_was() {
        let mut document = Document::parse(COMMENTED).unwrap();
        let error = document.replace("game = \n").unwrap_err();
        assert!(error.contains("line 1, column"), "{error}");
        assert_eq!(document.text(), COMMENTED);
        document.replace("game = \"avalon\"\n").unwrap();
        assert_eq!(document.text(), "game = \"avalon\"\n");
        assert!(Document::parse("[").is_err());
    }

    #[test]
    fn a_model_s_table_parses_as_a_game_parses_it() {
        let document = Document::parse(&example("werewolf/prompted.toml")).unwrap();
        let spec = document.model("qwen").unwrap();
        assert_eq!(spec.base_url, "http://localhost:1234/v1");
        assert_eq!(spec.id, "qwen2.5-7b-instruct-1m");
        let error = document.model("gpt").unwrap_err();
        assert_eq!(format!("{error:#}"), "there is no model gpt");
        let document = Document::parse("[models.bad]\nid = \"x\"\n").unwrap();
        let error = format!("{:#}", document.model("bad").unwrap_err());
        assert!(error.starts_with("[models.bad]: "), "{error}");
    }

    #[tokio::test]
    async fn a_document_with_a_broken_template_fails_to_prepare_naming_it() {
        let text = edited(
            &example("werewolf/prompted.toml"),
            &["strategies", "everyone", "system"],
            Value::Text("You are {{ name.\n".into()),
        );
        let error = format!("{:#}", prepare(&text, &Environment).await.err().unwrap());
        assert!(error.contains("everyone"), "{error}");
    }

    #[tokio::test]
    async fn an_invalid_duration_fails_to_prepare_at_its_line_and_column() {
        let text = edited(
            &example("werewolf/random.toml"),
            &["phases", "day_limit"],
            Value::Text("soon".into()),
        );
        let error = format!("{:#}", prepare(&text, &Environment).await.err().unwrap());
        let line = text.lines().position(|line| line.contains("soon")).unwrap() + 1;
        assert!(error.contains(&format!("line {line}, column")), "{error}");
        assert_eq!(social_deduction_stage::form::error_line(&error), Some(line));
    }
}
