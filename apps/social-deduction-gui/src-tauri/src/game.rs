//! The commands that play a game: the presets to choose from, playing
//! one, stopping it, and revealing its log.

use crate::secret::Keychain;
use serde::Serialize;
use social_deduction::live::{self, Playing, create_log};
use social_deduction::run::prepare;
use social_deduction_stage::Cue;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, async_runtime};

/// An example configuration to play.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    /// Where it is among the examples: `werewolf/random`.
    pub name: &'static str,
    /// The configuration file's text.
    pub text: &'static str,
}

/// The example configurations, in the order the frontend lists them.
const PRESETS: [Preset; 4] = [
    Preset {
        name: "werewolf/random",
        text: include_str!("../../../social-deduction/configs/werewolf/random.toml"),
    },
    Preset {
        name: "werewolf/prompted",
        text: include_str!("../../../social-deduction/configs/werewolf/prompted.toml"),
    },
    Preset {
        name: "avalon/random",
        text: include_str!("../../../social-deduction/configs/avalon/random.toml"),
    },
    Preset {
        name: "avalon/prompted",
        text: include_str!("../../../social-deduction/configs/avalon/prompted.toml"),
    },
];

/// The game the app is playing, or last played, with where its log is.
#[derive(Default)]
pub struct Game(Mutex<Option<(Playing, PathBuf)>>);

impl Game {
    /// Stop the game playing, if one is.
    pub fn stop(&self) {
        if let Some((playing, _)) = self.0.lock().unwrap().as_ref() {
            playing.stop();
        }
    }
}

/// Whether the game in `game` is playing now.
fn is_playing(game: &Option<(Playing, PathBuf)>) -> bool {
    game.as_ref()
        .is_some_and(|(playing, _)| !playing.is_finished())
}

/// The example configurations.
#[tauri::command]
pub fn presets() -> Vec<Preset> {
    PRESETS.to_vec()
}

/// Play the game the configuration `text` describes, its cues sent down
/// `cues`. The game is prepared first, with its keys from the keychain,
/// and a game that cannot be prepared is an error and never starts. A game
/// already playing refuses another.
#[tauri::command]
pub async fn play(
    app: AppHandle,
    game: State<'_, Game>,
    text: String,
    cues: Channel<Cue>,
) -> Result<(), String> {
    const PLAYING: &str = "A game is already playing.";
    if is_playing(&game.0.lock().unwrap()) {
        return Err(PLAYING.to_string());
    }
    // Preparing probes models through closures whose futures the compiler
    // cannot prove Send, so it runs to the end on a thread of its own,
    // with the runtime's handle.
    let prepared = async_runtime::spawn_blocking(move || {
        async_runtime::block_on(async move { prepare(&text, &Keychain).await })
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("{error:#}"))?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let (file, path) = create_log(&dir, prepared.game(), SystemTime::now())
        .map_err(|error| format!("The log could not be created: {error}"))?;
    let mut game = game.0.lock().unwrap();
    if is_playing(&game) {
        return Err(PLAYING.to_string());
    }
    let shown = Some(path.display().to_string());
    let playing = live::play(prepared, file, shown, move |cue| Ok(cues.send(cue)?));
    *game = Some((playing, path));
    Ok(())
}

/// Stop the game playing, if one is.
#[tauri::command]
pub fn stop(game: State<'_, Game>) {
    game.stop();
}

/// Reveal the last game's log in the system's file manager.
#[tauri::command]
pub fn show_log(game: State<'_, Game>) -> Result<(), String> {
    match game.0.lock().unwrap().as_ref() {
        Some((_, path)) => tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string()),
        None => Err("No game has been played.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use social_deduction::config;

    #[test]
    fn the_presets_are_the_four_examples_and_each_parses_as_the_game_it_names() {
        let names: Vec<&str> = presets().iter().map(|preset| preset.name).collect();
        assert_eq!(
            names,
            [
                "werewolf/random",
                "werewolf/prompted",
                "avalon/random",
                "avalon/prompted"
            ]
        );
        for preset in presets() {
            let game = match config::parse(preset.text).unwrap() {
                config::Settled::Werewolf(_) => "werewolf",
                config::Settled::Avalon(_) => "avalon",
            };
            assert!(preset.name.starts_with(game), "{}", preset.name);
        }
    }
}
