/*!
 * The ported game.
 *
 * This is the original's `main.rs` game loop, reshaped into a value the plugin
 * owns: [`Game::update`] runs one frame of logic, and [`Game::render`] produces
 * the frame the window presents. The heavy lifting — level parsing, player
 * physics, the camera, the software renderer, stats, and audio — lives in the
 * sibling modules, vendored from the original nearly verbatim.
 */

mod audio;
mod camera;
mod color;
mod input;
mod level;
mod player;
mod render;
mod screen;
mod stats;

pub use input::Input;

use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use image::RgbaImage;

use audio::SoundEffect;
use screen::{LevelScreen, Screen};
use stats::Stats;

/// The three levels, embedded from the original `levels/*.txt`.
const LEVELS: &[&str] = &[
    include_str!("../../levels/level1.txt"),
    include_str!("../../levels/level2.txt"),
    include_str!("../../levels/level3.txt"),
];

/// The game's internal resolution; the window scales this frame.
const WIDTH: usize = 360;
const HEIGHT: usize = 270;

/// Pixels below the level floor at which a fall counts as a death.
const DEATH_PLANE: f32 = 70.0;

/// How long a transient HUD message stays up, in seconds.
const MESSAGE_DURATION: f32 = 3.0;

/// The running game.
pub struct Game {
    screen: Option<Screen>,
    stats: Stats,
    audio: Option<audio::Audio>,
    effects_enabled: bool,
    finished: bool,
    screenshot_requested: bool,
    fps: f32,
    fps_frames: u32,
    fps_window: f32,
    message: Option<(String, f32)>,
}

impl Game {
    /// Start a new game at `start_level` (0-based). Audio is best-effort: a
    /// machine with no output device still plays, silently.
    pub fn new(start_level: usize, audio_enabled: bool, volume: u8) -> Result<Self> {
        let level_screen = load_level(start_level)?;
        let stats = Stats::new(start_level, LEVELS)?;
        let audio = if audio_enabled {
            audio::Audio::new("reverie-amidst-ruins", volume).ok()
        } else {
            None
        };

        Ok(Self {
            screen: Some(Screen::Title(level_screen)),
            stats,
            audio,
            effects_enabled: true,
            finished: false,
            screenshot_requested: false,
            fps: 0.0,
            fps_frames: 0,
            fps_window: 0.0,
            message: None,
        })
    }

    /// Advance one frame of logic by `dt`.
    pub fn update(&mut self, input: &Input, dt: Duration) -> Result<()> {
        let mut screen = self.screen.take().expect("the game always holds a screen");
        let delta = dt;

        // A one-second FPS window, as the original's HUD did. Measured from the
        // frame delta, not a wall clock, so it works on every target.
        let seconds = delta.as_secs_f32();
        self.fps_frames += 1;
        self.fps_window += seconds;
        if self.fps_window >= 1.0 {
            self.fps = self.fps_frames as f32 / self.fps_window;
            self.fps_frames = 0;
            self.fps_window = 0.0;
        }

        if input.effects_toggle_pressed {
            self.effects_enabled = !self.effects_enabled;
            let state = if self.effects_enabled { "on" } else { "off" };
            self.say(format!("effects {state}"));
        }
        if input.palette_randomize_pressed {
            let palette = color::random_palette();
            match &mut screen {
                Screen::Title(s) => s.level.palette = palette,
                Screen::Level(s) => s.level.palette = palette,
                Screen::End(s) => s.level.palette = palette,
            }
            self.say("palette randomized");
        }
        if input.screenshot_pressed {
            self.screenshot_requested = true;
        }
        if input.mute_toggle_pressed {
            let state = match &mut self.audio {
                Some(audio) => {
                    if audio.toggle_mute() {
                        "muted"
                    } else {
                        "unmuted"
                    }
                }
                None => "disabled",
            };
            self.say(format!("audio {state}"));
        }
        if input.volume_down_pressed || input.volume_up_pressed {
            let amount = if input.volume_down_pressed { -5 } else { 5 };
            let status = match &mut self.audio {
                Some(audio) => format!("volume {}%", audio.adjust_volume(amount)),
                None => "audio disabled".to_string(),
            };
            self.say(status);
        }

        // A restart (or a fall) resets the player and the level's coins.
        let restarted = if input.restart_pressed {
            if let Screen::Level(level_screen) = &mut screen {
                level_screen.player.reset();
                level_screen.remaining_coins = level_screen.level.coins.clone();
                level_screen.coin_count = 0;
                self.stats.deaths += 1;
                true
            } else {
                false
            }
        } else {
            false
        };

        // Confirm on the title screen starts the level.
        if input.confirm_pressed {
            screen = match screen {
                Screen::Title(level_screen) => {
                    if let Some(audio) = &mut self.audio {
                        audio.play_music(&level_screen.level.music)?;
                    }
                    Screen::Level(level_screen)
                }
                screen => screen,
            };
        }

        let mut next_level = None;
        let mut jumped = false;
        let mut collected_coin = false;
        let mut died = restarted;
        match &mut screen {
            Screen::Title(level_screen) | Screen::End(level_screen) => {
                level_screen.elapsed += delta.as_secs_f32();
            }
            Screen::Level(level_screen) => {
                self.stats.playtime += delta;
                level_screen.elapsed += delta.as_secs_f32();

                let level = &level_screen.level;
                let player = &mut level_screen.player;
                let level_width = (level.width * level::TILE_SIZE) as f32;

                let player_update = player.update(
                    input,
                    delta.as_secs_f32(),
                    &level.blocks,
                    level_width - player::HITBOX_WIDTH,
                );
                jumped = player_update.jumped;

                let level_height = (level.height * level::TILE_SIZE) as f32;
                if player.y > level_height + DEATH_PLANE {
                    player.reset();
                    level_screen.remaining_coins = level.coins.clone();
                    level_screen.coin_count = 0;
                    self.stats.deaths += 1;
                    died = true;
                } else {
                    level_screen.remaining_coins.retain(|&(x, y)| {
                        let collected = player.overlaps(
                            (x * level::TILE_SIZE) as f32,
                            (y * level::TILE_SIZE) as f32,
                            level::TILE_SIZE as f32,
                            level::TILE_SIZE as f32,
                        );
                        if collected {
                            level_screen.coin_count += 1;
                            collected_coin = true;
                        }
                        !collected
                    });

                    let goal = player.overlaps(
                        (level.goal.0 * level::TILE_SIZE) as f32,
                        (level.goal.1 * level::TILE_SIZE) as f32,
                        level::TILE_SIZE as f32,
                        level::TILE_SIZE as f32,
                    );
                    if goal {
                        self.stats.levels_beaten += 1;
                        self.stats.coins_collected += level_screen.coin_count;
                        if level_screen.index + 1 < LEVELS.len() {
                            next_level = Some(level_screen.index + 1);
                        } else {
                            self.finished = true;
                        }
                    }
                }

                level_screen.camera.follow(
                    player,
                    level_width,
                    level_height,
                    WIDTH as f32,
                    HEIGHT as f32,
                    delta.as_secs_f32(),
                );
            }
        }

        if let Some(index) = next_level {
            let level_screen = load_level(index)?;
            if let Some(audio) = &mut self.audio {
                audio.play_music(&level_screen.level.music)?;
            }
            screen = Screen::Level(level_screen);
        } else if self.finished {
            screen = match screen {
                Screen::Level(level_screen) => Screen::End(level_screen),
                screen => screen,
            };
        }

        if let Some(audio) = &mut self.audio {
            if jumped {
                audio.play_effect(SoundEffect::Jump);
            }
            if collected_coin {
                audio.play_effect(SoundEffect::Coin);
            }
            if died {
                audio.play_effect(SoundEffect::Dead);
            }
            audio.poll();
        }

        if let Some((_, remaining)) = &mut self.message {
            *remaining -= seconds;
        }
        if self.message.as_ref().is_some_and(|(_, left)| *left <= 0.0) {
            self.message = None;
        }

        self.screen = Some(screen);
        Ok(())
    }

    /// Set a transient HUD message.
    fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), MESSAGE_DURATION));
    }

    /// The screen-space HUD lines for the current state: the live level line, the
    /// end-screen summary, or a lone message on the title screen.
    pub fn hud_lines(&self) -> Vec<String> {
        let screen = self
            .screen
            .as_ref()
            .expect("the game always holds a screen");
        let message = self
            .message
            .as_ref()
            .map(|(text, _)| text.as_str())
            .unwrap_or("");

        match screen {
            Screen::Title(_) => {
                if message.is_empty() {
                    Vec::new()
                } else {
                    vec![message.to_string()]
                }
            }
            Screen::Level(level_screen) => {
                let mut line = format!(
                    "fps {:.0} | coins {} | deaths {}",
                    self.fps, level_screen.coin_count, self.stats.deaths
                );
                if !message.is_empty() {
                    line.push_str(" | ");
                    line.push_str(message);
                }
                vec![line]
            }
            Screen::End(_) => {
                let mut lines = self.stats.summary_lines();
                if !message.is_empty() {
                    lines.push(message.to_string());
                }
                lines
            }
        }
    }

    /// Render the current frame, apply the post-process pass, and save a
    /// screenshot if one was requested this frame.
    pub fn render(&mut self) -> Result<RgbaImage> {
        let screen = self
            .screen
            .as_mut()
            .expect("the game always holds a screen");

        let mut frame = match screen {
            Screen::Title(level_screen) => {
                let mut frame = render::level(
                    &level_screen.level,
                    None,
                    &level_screen.camera,
                    &level_screen.remaining_coins,
                    level_screen.elapsed,
                    WIDTH as u32,
                    HEIGHT as u32,
                )?;
                render::title(&mut frame)?;
                frame
            }
            Screen::Level(level_screen) => render::level(
                &level_screen.level,
                Some(&level_screen.player),
                &level_screen.camera,
                &level_screen.remaining_coins,
                level_screen.elapsed,
                WIDTH as u32,
                HEIGHT as u32,
            )?,
            Screen::End(level_screen) => {
                let mut frame = render::level(
                    &level_screen.level,
                    None,
                    &level_screen.camera,
                    &level_screen.remaining_coins,
                    level_screen.elapsed,
                    WIDTH as u32,
                    HEIGHT as u32,
                )?;
                render::end(&mut frame)?;
                frame
            }
        };

        if self.effects_enabled {
            render::post_process(&mut frame);
        }

        if self.screenshot_requested {
            self.screenshot_requested = false;
            // No filesystem on the web; screenshots are a native nicety.
            #[cfg(not(target_arch = "wasm32"))]
            {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .context("system clock is bad")?
                    .as_millis();
                let filename = format!("rar-screenshot-{timestamp}.png");
                frame
                    .save(&filename)
                    .with_context(|| format!("could not save {filename}"))?;
                self.say(format!("saved {filename}"));
            }
        }

        Ok(frame)
    }
}

/// Load level `index`, snapping the camera to the spawn.
fn load_level(index: usize) -> Result<LevelScreen> {
    let source = LEVELS.get(index).context("unknown level")?;

    let mut level_screen = LevelScreen::load(index, source)
        .with_context(|| format!("could not load level {}", index + 1))?;

    let level_width = (level_screen.level.width * level::TILE_SIZE) as f32;
    let level_height = (level_screen.level.height * level::TILE_SIZE) as f32;
    level_screen.camera.snap_to(
        &level_screen.player,
        level_width,
        level_height,
        WIDTH as f32,
        HEIGHT as f32,
    );

    Ok(level_screen)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const STEP: Duration = Duration::from_millis(16);

    #[test]
    fn every_embedded_level_parses() {
        for (index, source) in LEVELS.iter().enumerate() {
            let level = level::LevelDefinition::parse(source)
                .unwrap_or_else(|error| panic!("level {} parses: {error}", index + 1));
            assert!(!level.coins.is_empty(), "level {} has coins", index + 1);
            assert!(level.width > 0 && level.height > 0);
        }
    }

    #[test]
    fn the_game_runs_and_renders_real_frames() {
        let mut game = Game::new(0, false, 0).expect("the game starts");
        let idle = Input::default();

        // The title screen renders at the game's resolution and has no HUD.
        for _ in 0..10 {
            game.update(&idle, STEP).expect("tick the title screen");
        }
        let title = game.render().expect("render the title");
        assert_eq!(
            (title.width(), title.height()),
            (WIDTH as u32, HEIGHT as u32)
        );
        assert!(game.hud_lines().is_empty(), "the title screen has no HUD");

        // Confirm starts the level; let the player fall and land.
        let confirm = Input {
            confirm_pressed: true,
            ..Input::default()
        };
        game.update(&confirm, STEP).expect("confirm the title");
        for _ in 0..120 {
            game.update(&idle, STEP).expect("tick the level");
        }
        let frame = game.render().expect("render the level");
        assert_eq!(
            (frame.width(), frame.height()),
            (WIDTH as u32, HEIGHT as u32)
        );

        // The level HUD reports fps, coins, and deaths.
        let hud = game.hud_lines();
        assert!(
            hud.iter()
                .any(|line| line.contains("coins") && line.contains("deaths")),
            "unexpected HUD: {hud:?}"
        );
        assert_eq!(
            (frame.width(), frame.height()),
            (WIDTH as u32, HEIGHT as u32)
        );

        // Geometry, not a flat clear: the frame must hold several colours.
        let colours: HashSet<[u8; 4]> = frame.pixels().map(|pixel| pixel.0).collect();
        assert!(
            colours.len() > 4,
            "the level rendered only {} colour(s)",
            colours.len()
        );
    }
}
