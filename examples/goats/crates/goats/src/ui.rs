//! The game's menus and console (M6), drawn on the engine's screen-UI seam
//! ([`.notes/ui.md`](../../../../.notes/ui.md)).
//!
//! Ported from the original's `menu.js` and `console.js`. The original draws with
//! raygui and hand-rolls the console's caret, history and wrapping; egui gives us
//! those, so this is the screens themselves plus a small command queue. A menu
//! freezes the world (the plugin skips `apply_frame` while one is open) and the
//! game ignores its input, exactly as the original's `uiScreen` does.

use noctis::prelude::*;

use crate::weather;

/// The keymap table (`KEYMAP_ROWS`), the port's own bindings.
pub const KEYMAP: &[(&str, &str)] = &[
    ("W / S", "walk forward / back"),
    ("CTRL + W/S", "trot"),
    ("SHIFT + W/S", "run"),
    ("A / D", "turn left / right"),
    ("SPACE", "jump"),
    ("E", "eat the grass in reach"),
    ("Z", "sleep / wake"),
    ("R", "restart after death"),
    ("C", "next weather"),
    ("J", "toggle the water"),
    ("M", "mute audio"),
    ("8 / 9", "volume down / up"),
    ("Mouse drag", "orbit the camera"),
    ("Mouse wheel", "zoom"),
    ("Arrow keys", "orbit the camera"),
    ("P / ESC", "pause / resume"),
    ("Enter", "run the console line"),
];

/// How many console lines the panel shows (the newest).
const CONSOLE_LINES: usize = 12;

/// Which screen is showing. `Hud` is the game itself; any other freezes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Screen {
    /// No menu: the game runs and its HUD shows.
    #[default]
    Hud,
    /// The pause menu.
    Main,
    /// The audio / world settings.
    Settings,
    /// The read-only keymap table.
    Keymap,
    /// The command console.
    Console,
}

impl Screen {
    /// Whether a screen is up, so the world is frozen and the game ignores input.
    pub fn open(self) -> bool {
        self != Screen::Hud
    }
}

/// A command a screen asked for that only the plugin can carry out — the audio and
/// the loop. The world's own commands (weather, water, restart) are applied by
/// [`World::draw_ui`](crate::World::draw_ui), which owns them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginCommand {
    /// Toggle mute (`M`, or the settings checkbox).
    Mute,
    /// Set the master volume, `0..=100` (the settings slider).
    Volume(u8),
    /// Quit the game.
    Quit,
}

/// A command a screen asked for that the world itself carries out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WorldCommand {
    /// Restart the goat where it stands (`R`).
    Restart,
    /// Grow or shrink the herd (`herd.count`).
    Herd(usize),
    /// Force the weather: the named state, or the next one (`C`).
    Weather(Option<weather::Kind>),
    /// Pin the water table to a level, or hand it back to the rain.
    Flood(Option<f32>),
    /// Turn the whole water system on or off (`J`).
    Water(bool),
}

/// The parse of one console line.
enum Verb {
    Help,
    Clear,
    World(WorldCommand),
    Plugin(PluginCommand),
    Error(String),
}

/// The menus' state and the values the screens show (M6). Lives in the
/// [`World`](crate::World) so `apply_frame` can publish the view into it.
#[derive(Default)]
pub struct Menus {
    /// The screen that is showing.
    pub screen: Screen,
    /// Whether the game has run a frame yet (the `Continue`/`Play` label).
    pub started: bool,
    /// The frame rate, for the menu's footer.
    pub fps: f32,
    /// The goat's state label.
    pub goat: String,
    /// The weather line.
    pub weather: String,
    /// How many goats are in the herd.
    pub herd: usize,
    /// The master volume, `0..=100` — synced with the plugin's audio each frame.
    pub volume: u8,
    /// Whether audio is muted — synced with the plugin's audio each frame.
    pub muted: bool,
    /// Whether the water system is on — synced with the world each frame.
    pub water_on: bool,
    /// The console's current line.
    pub input: String,
    /// The console's scrollback, oldest first.
    pub log: Vec<String>,
    world_commands: Vec<WorldCommand>,
    plugin_commands: Vec<PluginCommand>,
}

impl Menus {
    /// A fresh menu state: the game runs, nothing is queued.
    pub fn new() -> Menus {
        Menus {
            volume: 100,
            water_on: true,
            log: vec!["goats console — `help` for the verbs".to_string()],
            ..Menus::default()
        }
    }

    /// Whether a screen is up.
    pub fn is_open(&self) -> bool {
        self.screen.open()
    }

    /// Show `screen`.
    pub fn open(&mut self, screen: Screen) {
        self.screen = screen;
    }

    /// `Esc`/`P`: open the main menu, or close the current screen.
    pub fn toggle_pause(&mut self) {
        self.screen = if self.screen.open() {
            Screen::Hud
        } else {
            Screen::Main
        };
    }

    /// Add a line to the console's scrollback.
    pub fn push(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        let overflow = self.log.len().saturating_sub(CONSOLE_LINES.max(64));
        if overflow > 0 {
            self.log.drain(0..overflow);
        }
    }

    /// Run the console's current line (the plugin calls this on `Enter`).
    pub fn submit(&mut self) {
        let line = std::mem::take(&mut self.input);
        let line = line.trim().to_string();
        if line.is_empty() {
            return;
        }
        self.push(format!("> {line}"));
        match parse(&line) {
            Verb::Help => {
                for (verb, what) in VERBS {
                    self.push(format!("  {verb:<26} {what}"));
                }
            }
            Verb::Clear => self.log.clear(),
            Verb::World(command) => self.world_commands.push(command),
            Verb::Plugin(command) => self.plugin_commands.push(command),
            Verb::Error(message) => self.push(message),
        }
    }

    /// The plugin's own commands, drained (audio, quit).
    pub fn take_plugin_commands(&mut self) -> Vec<PluginCommand> {
        std::mem::take(&mut self.plugin_commands)
    }

    /// Apply the world's commands to `world` by hand — the crate-internal half of
    /// [`World::draw_ui`](crate::World::draw_ui).
    pub(crate) fn drain_world_commands(&mut self) -> Vec<WorldCommand> {
        std::mem::take(&mut self.world_commands)
    }

    /// Draw the active screen into the seam's `Ui`.
    pub fn draw(&mut self, ui: &mut dyn Ui) {
        match self.screen {
            Screen::Hud => {}
            Screen::Main => self.draw_main(ui),
            Screen::Settings => self.draw_settings(ui),
            Screen::Keymap => self.draw_keymap(ui),
            Screen::Console => self.draw_console(ui),
        }
    }

    fn draw_main(&mut self, ui: &mut dyn Ui) {
        let playing = self.started;
        let foot = format!("{:.0} fps   |   {}", self.fps, self.goat);
        ui.panel("goats.main", "Slag goat", &mut |ui| {
            if ui.button(if playing { "Continue" } else { "Play" }) {
                self.screen = Screen::Hud;
            }
            if ui.button("Restart") {
                self.world_commands.push(WorldCommand::Restart);
                self.screen = Screen::Hud;
            }
            if ui.button("Settings") {
                self.screen = Screen::Settings;
            }
            if ui.button("Keymap") {
                self.screen = Screen::Keymap;
            }
            if ui.button("Console") {
                self.screen = Screen::Console;
            }
            if ui.button("Quit") {
                self.plugin_commands.push(PluginCommand::Quit);
            }
            ui.separator();
            ui.label(&foot);
            ui.label(&self.weather);
        });
    }

    fn draw_settings(&mut self, ui: &mut dyn Ui) {
        let mut volume = self.volume as f32;
        let mut muted = self.muted;
        let mut water = self.water_on;
        let mut herd = self.herd as f32;
        let weather = self.weather.clone();
        let mut back = false;
        ui.panel("goats.settings", "Settings", &mut |ui| {
            ui.label("Audio");
            if ui.slider("Volume", &mut volume, 0.0, 100.0) {
                self.plugin_commands
                    .push(PluginCommand::Volume(volume.round() as u8));
            }
            if ui.toggle("Mute", &mut muted) {
                self.plugin_commands.push(PluginCommand::Mute);
            }
            ui.separator();
            ui.label("World");
            if ui.toggle("Water", &mut water) {
                self.world_commands.push(WorldCommand::Water(water));
            }
            if ui.slider("Herd size", &mut herd, 0.0, 100.0) {
                self.world_commands
                    .push(WorldCommand::Herd(herd.round() as usize));
            }
            ui.label(&weather);
            if ui.button("Next weather") {
                self.world_commands.push(WorldCommand::Weather(None));
            }
            ui.separator();
            if ui.button("Back") {
                back = true;
            }
        });
        if back {
            self.screen = Screen::Main;
        }
    }

    fn draw_keymap(&mut self, ui: &mut dyn Ui) {
        let mut back = false;
        ui.panel("goats.keymap", "Keymap", &mut |ui| {
            for (key, action) in KEYMAP {
                ui.label(&format!("{key:<14} {action}"));
            }
            ui.separator();
            if ui.button("Back") {
                back = true;
            }
        });
        if back {
            self.screen = Screen::Main;
        }
    }

    fn draw_console(&mut self, ui: &mut dyn Ui) {
        let start = self.log.len().saturating_sub(CONSOLE_LINES);
        let mut back = false;
        ui.panel("goats.console", "Console", &mut |ui| {
            for line in &self.log[start..] {
                ui.label(line);
            }
            ui.separator();
            let _ = ui.text_field("goats.console.input", &mut self.input);
            ui.label("Enter runs it; `help` lists the verbs");
            if ui.button("Close") {
                back = true;
            }
        });
        if back {
            self.screen = Screen::Hud;
        }
    }
}

/// The console's verbs, for `help` and for anyone reading the panel.
const VERBS: &[(&str, &str)] = &[
    ("help", "list these verbs"),
    ("clear", "empty the console"),
    ("restart", "start the goat's life over"),
    ("herd [0-10]", "set the herd size"),
    (
        "weather [state]",
        "force the weather: clear, cloudy, rain, clearing",
    ),
    (
        "flood [off|metres]",
        "pin the water table, or hand it back to the rain",
    ),
    ("water [on|off]", "turn the whole water system on or off"),
    ("mute", "mute or unmute the audio"),
    ("volume [0-100]", "set the master volume"),
    ("quit", "leave the game"),
];

/// Parse one console line (`console.js`'s verb table, in the port's own terms).
fn parse(line: &str) -> Verb {
    let mut parts = line.split_whitespace();
    let verb = parts.next().unwrap_or("help").to_ascii_lowercase();
    let arg = parts.next();
    match verb.as_str() {
        "help" | "?" => Verb::Help,
        "clear" => Verb::Clear,
        "restart" | "r" => Verb::World(WorldCommand::Restart),
        "quit" | "q" | "exit" => Verb::Plugin(PluginCommand::Quit),
        "mute" => Verb::Plugin(PluginCommand::Mute),
        "volume" | "vol" => match arg.and_then(|a| a.parse::<f32>().ok()) {
            Some(v) => Verb::Plugin(PluginCommand::Volume(v.clamp(0.0, 100.0) as u8)),
            None => Verb::Error("volume needs a number, 0-100".to_string()),
        },
        "herd" => match arg.and_then(|a| a.parse::<usize>().ok()) {
            Some(n) => Verb::World(WorldCommand::Herd(n)),
            None => Verb::Error("herd needs a count, 0-10".to_string()),
        },
        "weather" | "w" => {
            let kind = match arg.map(|a| a.to_ascii_lowercase()) {
                None => None,
                Some(a) if a == "next" => None,
                Some(a) => match weather::Kind::from_name(&a) {
                    Some(kind) => Some(kind),
                    None => return Verb::Error(format!("unknown weather '{a}'")),
                },
            };
            Verb::World(WorldCommand::Weather(kind))
        }
        "flood" | "f" => match arg.map(|a| a.to_ascii_lowercase()) {
            None => Verb::World(WorldCommand::Flood(None)),
            Some(a) if a == "off" => Verb::World(WorldCommand::Flood(None)),
            Some(a) => match a.parse::<f32>() {
                Ok(level) => Verb::World(WorldCommand::Flood(Some(level))),
                Err(_) => Verb::Error(format!("flood needs a height or 'off', not '{a}'")),
            },
        },
        "water" | "j" => match arg.map(|a| a.to_ascii_lowercase()).as_deref() {
            None => Verb::World(WorldCommand::Water(true)),
            Some("on") | Some("1") | Some("true") => Verb::World(WorldCommand::Water(true)),
            Some("off") | Some("0") | Some("false") => Verb::World(WorldCommand::Water(false)),
            Some(other) => Verb::Error(format!("water needs on or off, not '{other}'")),
        },
        other => Verb::Error(format!("unknown verb '{other}' — try `help`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_menu_navigates_and_queues_a_quit() {
        let mut menus = Menus::new();
        menus.open(Screen::Main);
        let mut ui = RecordingUi::new();
        ui.press("Settings");
        menus.draw(&mut ui);
        assert_eq!(menus.screen, Screen::Settings, "Settings opened");

        let mut ui = RecordingUi::new();
        ui.press("Back");
        menus.draw(&mut ui);
        assert_eq!(menus.screen, Screen::Main, "Back returned");

        let mut ui = RecordingUi::new();
        ui.press("Quit");
        menus.draw(&mut ui);
        assert_eq!(menus.take_plugin_commands(), vec![PluginCommand::Quit]);
    }

    #[test]
    fn the_settings_queue_their_commands() {
        let mut menus = Menus::new();
        menus.screen = Screen::Settings;
        let mut ui = RecordingUi::new();
        ui.press("Volume");
        ui.press("Water");
        ui.press("Herd size");
        menus.draw(&mut ui);

        let plugin = menus.take_plugin_commands();
        assert!(
            plugin.iter().any(|c| matches!(c, PluginCommand::Volume(_))),
            "a volume was queued: {plugin:?}"
        );
        let world = menus.drain_world_commands();
        assert!(
            world.iter().any(|c| matches!(c, WorldCommand::Water(_))),
            "the water toggle queued a world command: {world:?}"
        );
        assert!(
            world.iter().any(|c| matches!(c, WorldCommand::Herd(_))),
            "the herd slider queued a world command: {world:?}"
        );
    }

    #[test]
    fn the_keymap_lists_the_port_bindings() {
        let mut menus = Menus::new();
        menus.screen = Screen::Keymap;
        let mut ui = RecordingUi::new();
        menus.draw(&mut ui);
        assert!(ui.drew("label:SPACE"), "the keymap table drew");
        assert!(ui.drew("panel:goats.keymap/Keymap"));
    }

    #[test]
    fn the_console_parses_the_verbs() {
        let mut menus = Menus::new();

        menus.input = "weather rain".to_string();
        menus.submit();
        assert!(matches!(
            menus.drain_world_commands().as_slice(),
            [WorldCommand::Weather(Some(weather::Kind::Rain))]
        ));

        menus.input = "flood 0.4".to_string();
        menus.submit();
        assert!(matches!(
            menus.drain_world_commands().as_slice(),
            [WorldCommand::Flood(Some(level))] if (level - 0.4).abs() < 1e-3
        ));

        menus.input = "flood off".to_string();
        menus.submit();
        assert!(matches!(
            menus.drain_world_commands().as_slice(),
            [WorldCommand::Flood(None)]
        ));

        menus.input = "mute".to_string();
        menus.submit();
        assert_eq!(menus.take_plugin_commands(), vec![PluginCommand::Mute]);

        menus.input = "volume 40".to_string();
        menus.submit();
        assert_eq!(
            menus.take_plugin_commands(),
            vec![PluginCommand::Volume(40)]
        );

        menus.input = "herd 4".to_string();
        menus.submit();
        assert!(matches!(
            menus.drain_world_commands().as_slice(),
            [WorldCommand::Herd(4)]
        ));

        menus.input = "help".to_string();
        menus.submit();
        assert!(
            menus.log.iter().any(|line| line.contains("weather")),
            "help"
        );

        menus.input = "wat".to_string();
        menus.submit();
        assert!(
            menus.log.iter().any(|line| line.contains("unknown verb")),
            "an unknown verb is reported: {:?}",
            menus.log
        );
    }
}
