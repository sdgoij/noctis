/*!
 * The game's input, mapped from the engine's keyboard state.
 *
 * The original read `crossterm` key events (and a gamepad); here the engine
 * delivers an [`InputState`] each frame, which we fold into the same shape the
 * rest of the game already expects.
 */

use noctis::prelude::{InputState, Key};

/// The game's per-frame input. The one-shot `*_pressed` flags are true only on
/// the frame the key went down; the held flags stay true while it is down.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub jump_pressed: bool,
    pub confirm_pressed: bool,
    pub screenshot_pressed: bool,
    pub effects_toggle_pressed: bool,
    pub palette_randomize_pressed: bool,
    pub restart_pressed: bool,
    pub mute_toggle_pressed: bool,
    pub volume_down_pressed: bool,
    pub volume_up_pressed: bool,
    pub quit: bool,
}

impl Input {
    /// Fold one frame of engine keyboard state into the game's input.
    pub fn from_state(state: &InputState) -> Self {
        let down = |key: Key| state.key_down(key);
        let pressed = |key: Key| state.key_pressed(key);
        let letter = |c: char| Key::letter(c).expect("an ASCII letter key");
        let digit = |c: char| Key::digit(c).expect("an ASCII digit key");

        Self {
            left: down(letter('A')) || down(Key::ArrowLeft),
            right: down(letter('D')) || down(Key::ArrowRight),
            jump: down(Key::Space) || down(Key::ArrowUp) || down(letter('W')),
            jump_pressed: pressed(Key::Space) || pressed(Key::ArrowUp) || pressed(letter('W')),
            confirm_pressed: pressed(Key::Enter),
            screenshot_pressed: pressed(digit('2')),
            effects_toggle_pressed: pressed(digit('1')),
            palette_randomize_pressed: pressed(digit('3')),
            restart_pressed: pressed(letter('R')),
            mute_toggle_pressed: pressed(letter('M')),
            volume_down_pressed: pressed(digit('8')),
            volume_up_pressed: pressed(digit('9')),
            quit: down(Key::Escape) || down(letter('Q')),
        }
    }
}
