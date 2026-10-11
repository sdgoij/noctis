//! Global resources: time, input, action mapping, and events.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Frame timing and the fixed-step accumulator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Time {
    delta: f32,
    elapsed: f32,
    frame: u64,
    fixed_delta: f32,
    accumulator: f32,
    scale: f32,
    max_delta: f32,
}

impl Default for Time {
    fn default() -> Self {
        Self::new()
    }
}

impl Time {
    /// A clock with a 60 Hz fixed step and a 0.25 s frame-time clamp.
    pub fn new() -> Self {
        Self {
            delta: 0.0,
            elapsed: 0.0,
            frame: 0,
            fixed_delta: 1.0 / 60.0,
            accumulator: 0.0,
            scale: 1.0,
            max_delta: 0.25,
        }
    }

    /// Advance the clock by a raw (unscaled) frame time in seconds.
    pub fn advance(&mut self, raw_delta: f32) {
        let clamped = raw_delta.clamp(0.0, self.max_delta);
        self.delta = clamped * self.scale;
        self.elapsed += self.delta;
        self.accumulator += self.delta;
        self.frame += 1;
    }

    /// Pull one fixed step from the accumulator, or `None` if not enough time
    /// has accrued.
    pub fn step_fixed(&mut self) -> Option<f32> {
        if self.fixed_delta > 0.0 && self.accumulator >= self.fixed_delta {
            self.accumulator -= self.fixed_delta;
            Some(self.fixed_delta)
        } else {
            None
        }
    }

    /// The scaled delta time of the last frame, seconds.
    #[inline]
    pub fn delta(&self) -> f32 {
        self.delta
    }

    /// Total scaled time since the clock was created, seconds.
    #[inline]
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// The number of frames advanced.
    #[inline]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// The fixed timestep, seconds.
    #[inline]
    pub fn fixed_delta(&self) -> f32 {
        self.fixed_delta
    }

    /// The time scale (1.0 is real time).
    #[inline]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Set the time scale. Values are clamped to be non-negative.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.max(0.0);
    }

    /// The fraction of a fixed step already accumulated, for interpolating
    /// rendered state between physics steps.
    pub fn interpolation_alpha(&self) -> f32 {
        if self.fixed_delta > 0.0 {
            (self.accumulator / self.fixed_delta).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// A keyboard key. A pragmatic subset covering typical game controls; unknown
/// keys can be represented by the scripting layer's numeric codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// `A`–`Z`. The `u8` is `0..=25` for `A..=Z`.
    Letter(u8),
    /// `0`–`9`. The `u8` is `0..=9`.
    Digit(u8),
    /// Space bar.
    Space,
    /// Enter / Return.
    Enter,
    /// Escape.
    Escape,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Left Shift.
    ShiftLeft,
    /// Right Shift.
    ShiftRight,
    /// Left Control.
    ControlLeft,
    /// Right Control.
    ControlRight,
    /// Left Alt.
    AltLeft,
    /// Right Alt.
    AltRight,
    /// Up arrow.
    ArrowUp,
    /// Down arrow.
    ArrowDown,
    /// Left arrow.
    ArrowLeft,
    /// Right arrow.
    ArrowRight,
}

impl Key {
    /// A letter key from an ASCII uppercase letter.
    pub fn letter(c: char) -> Option<Self> {
        c.to_ascii_uppercase()
            .is_ascii_uppercase()
            .then(|| Key::Letter(c.to_ascii_uppercase() as u8 - b'A'))
    }

    /// A digit key from a digit character.
    pub fn digit(c: char) -> Option<Self> {
        c.to_digit(10).map(|d| Key::Digit(d as u8))
    }

    /// The canonical name, matching the scripting API's key names.
    pub fn name(self) -> String {
        match self {
            Key::Letter(i) => format!("Key{}", (b'A' + i) as char),
            Key::Digit(d) => format!("Digit{d}"),
            Key::Space => "Space".into(),
            Key::Enter => "Enter".into(),
            Key::Escape => "Escape".into(),
            Key::Tab => "Tab".into(),
            Key::Backspace => "Backspace".into(),
            Key::ShiftLeft => "ShiftLeft".into(),
            Key::ShiftRight => "ShiftRight".into(),
            Key::ControlLeft => "ControlLeft".into(),
            Key::ControlRight => "ControlRight".into(),
            Key::AltLeft => "AltLeft".into(),
            Key::AltRight => "AltRight".into(),
            Key::ArrowUp => "ArrowUp".into(),
            Key::ArrowDown => "ArrowDown".into(),
            Key::ArrowLeft => "ArrowLeft".into(),
            Key::ArrowRight => "ArrowRight".into(),
        }
    }

    /// Parse a key from its canonical name.
    pub fn from_name(name: &str) -> Option<Self> {
        if let Some(rest) = name.strip_prefix("Key") {
            let c = rest.chars().next()?;
            return (rest.len() == 1).then(|| Key::letter(c)).flatten();
        }
        if let Some(rest) = name.strip_prefix("Digit") {
            let c = rest.chars().next()?;
            return (rest.len() == 1).then(|| Key::digit(c)).flatten();
        }
        Some(match name {
            "Space" => Key::Space,
            "Enter" => Key::Enter,
            "Escape" => Key::Escape,
            "Tab" => Key::Tab,
            "Backspace" => Key::Backspace,
            "ShiftLeft" => Key::ShiftLeft,
            "ShiftRight" => Key::ShiftRight,
            "ControlLeft" => Key::ControlLeft,
            "ControlRight" => Key::ControlRight,
            "AltLeft" => Key::AltLeft,
            "AltRight" => Key::AltRight,
            "ArrowUp" => Key::ArrowUp,
            "ArrowDown" => Key::ArrowDown,
            "ArrowLeft" => Key::ArrowLeft,
            "ArrowRight" => Key::ArrowRight,
            _ => return None,
        })
    }
}

impl Serialize for Key {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as the canonical name so bindings survive enum changes.
        serializer.serialize_str(&self.name())
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        Key::from_name(&name)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown key name {name:?}")))
    }
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// The left button.
    Left,
    /// The right button.
    Right,
    /// The middle button.
    Middle,
}

impl MouseButton {
    fn index(self) -> usize {
        match self {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        }
    }

    /// The canonical (lowercase) name, used in serialized bindings.
    pub fn name(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        }
    }

    /// Parse a mouse button from its canonical name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "left" => Some(MouseButton::Left),
            "right" => Some(MouseButton::Right),
            "middle" => Some(MouseButton::Middle),
            _ => None,
        }
    }
}

impl Serialize for MouseButton {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for MouseButton {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        MouseButton::from_name(&name)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown mouse button {name:?}")))
    }
}

/// The state of the mouse.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouseState {
    /// The cursor position in logical pixels.
    pub position: [f32; 2],
    /// The movement since the previous frame.
    pub delta: [f32; 2],
    /// The scroll wheel delta since the previous frame.
    pub scroll: f32,
    down: [bool; 3],
    pressed: [bool; 3],
    released: [bool; 3],
}

impl MouseState {
    /// `true` if the button is held.
    pub fn is_down(&self, button: MouseButton) -> bool {
        self.down[button.index()]
    }

    /// `true` if the button was pressed this frame.
    pub fn is_pressed(&self, button: MouseButton) -> bool {
        self.pressed[button.index()]
    }

    /// `true` if the button was released this frame.
    pub fn is_released(&self, button: MouseButton) -> bool {
        self.released[button.index()]
    }
}

/// Aggregated keyboard and mouse state for the current frame.
#[derive(Clone, Debug, Default)]
pub struct InputState {
    keys_down: HashSet<Key>,
    keys_pressed: HashSet<Key>,
    keys_released: HashSet<Key>,
    /// The window's inner size in logical pixels, so a script can map the cursor
    /// into a 2D scene. Zero until the platform reports one.
    viewport: [f32; 2],
    /// The mouse state.
    pub mouse: MouseState,
}

impl InputState {
    /// An empty input state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear per-frame edge state. Call once at the start of each frame, before
    /// feeding events.
    pub fn begin_frame(&mut self) {
        self.keys_pressed.clear();
        self.keys_released.clear();
        self.mouse.pressed = [false; 3];
        self.mouse.released = [false; 3];
        self.mouse.delta = [0.0, 0.0];
        self.mouse.scroll = 0.0;
    }

    /// Record a key press.
    pub fn press_key(&mut self, key: Key) {
        if self.keys_down.insert(key) {
            self.keys_pressed.insert(key);
        }
    }

    /// Record a key release.
    pub fn release_key(&mut self, key: Key) {
        if self.keys_down.remove(&key) {
            self.keys_released.insert(key);
        }
    }

    /// `true` if the key is held.
    pub fn key_down(&self, key: Key) -> bool {
        self.keys_down.contains(&key)
    }

    /// `true` if the key was pressed this frame.
    pub fn key_pressed(&self, key: Key) -> bool {
        self.keys_pressed.contains(&key)
    }

    /// `true` if the key was released this frame.
    pub fn key_released(&self, key: Key) -> bool {
        self.keys_released.contains(&key)
    }

    /// Record a mouse button press.
    pub fn press_mouse(&mut self, button: MouseButton) {
        let i = button.index();
        if !self.mouse.down[i] {
            self.mouse.down[i] = true;
            self.mouse.pressed[i] = true;
        }
    }

    /// Record a mouse button release.
    pub fn release_mouse(&mut self, button: MouseButton) {
        let i = button.index();
        if self.mouse.down[i] {
            self.mouse.down[i] = false;
            self.mouse.released[i] = true;
        }
    }

    /// Move the cursor, updating the frame delta.
    pub fn set_mouse_position(&mut self, x: f32, y: f32) {
        self.mouse.delta[0] += x - self.mouse.position[0];
        self.mouse.delta[1] += y - self.mouse.position[1];
        self.mouse.position = [x, y];
    }

    /// Add scroll wheel movement.
    pub fn add_scroll(&mut self, amount: f32) {
        self.mouse.scroll += amount;
    }

    /// Add raw pointer motion (a device delta), updating the frame delta without
    /// an absolute position.
    ///
    /// Used while the cursor is captured, when the OS reports motion as a delta
    /// rather than as a new cursor position.
    pub fn add_mouse_motion(&mut self, dx: f32, dy: f32) {
        self.mouse.delta[0] += dx;
        self.mouse.delta[1] += dy;
    }

    /// Record the window's inner size (logical pixels).
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport = [width as f32, height as f32];
    }

    /// The window's inner size in logical pixels, defaulting to `1280x720` until
    /// the platform reports one.
    pub fn viewport(&self) -> [f32; 2] {
        if self.viewport[0] > 0.0 && self.viewport[1] > 0.0 {
            self.viewport
        } else {
            [1280.0, 720.0]
        }
    }

    /// A `-1..=1` axis from two keys: `+1` if `positive` is held, `-1` if
    /// `negative` is held, `0` otherwise.
    pub fn axis(&self, negative: Key, positive: Key) -> f32 {
        (self.key_down(positive) as i32 - self.key_down(negative) as i32) as f32
    }
}

/// A named action bound to one or more keys.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActionMap {
    bindings: HashMap<String, Vec<Key>>,
}

impl ActionMap {
    /// An empty action map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind keys to an action, replacing any existing binding.
    pub fn bind(&mut self, action: impl Into<String>, keys: impl IntoIterator<Item = Key>) {
        self.bindings
            .insert(action.into(), keys.into_iter().collect());
    }

    /// The keys bound to an action.
    pub fn keys(&self, action: &str) -> &[Key] {
        self.bindings.get(action).map_or(&[], Vec::as_slice)
    }

    /// `true` if any bound key is held.
    pub fn is_down(&self, action: &str, input: &InputState) -> bool {
        self.keys(action).iter().any(|k| input.key_down(*k))
    }

    /// `true` if any bound key was pressed this frame.
    pub fn is_pressed(&self, action: &str, input: &InputState) -> bool {
        self.keys(action).iter().any(|k| input.key_pressed(*k))
    }

    /// A `-1..=1` axis from two actions.
    pub fn axis(&self, negative: &str, positive: &str, input: &InputState) -> f32 {
        (self.is_down(positive, input) as i32 - self.is_down(negative, input) as i32) as f32
    }
}

/// A double-buffered event queue.
///
/// Events sent during a frame are visible to readers for the following frame,
/// then dropped on the next [`Events::update`]. This avoids the "events must be
/// read in the same frame they are sent" trap.
#[derive(Clone, Debug, Default)]
pub struct Events<E> {
    current: Vec<E>,
    previous: Vec<E>,
}

impl<E> Events<E> {
    /// An empty queue.
    pub fn new() -> Self {
        Self {
            current: Vec::new(),
            previous: Vec::new(),
        }
    }

    /// Send an event.
    pub fn send(&mut self, event: E) {
        self.current.push(event);
    }

    /// Iterate the events from the previous frame.
    pub fn read(&self) -> impl Iterator<Item = &E> {
        self.previous.iter()
    }

    /// The number of readable events.
    pub fn len(&self) -> usize {
        self.previous.len()
    }

    /// `true` if there are no readable events.
    pub fn is_empty(&self) -> bool {
        self.previous.is_empty()
    }

    /// Rotate buffers. Call once per frame after systems have read events.
    pub fn update(&mut self) {
        self.previous = std::mem::take(&mut self.current);
    }

    /// Drop all pending and readable events.
    pub fn clear(&mut self) {
        self.current.clear();
        self.previous.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_advance_and_clamp() {
        let mut t = Time::new();
        t.advance(0.016);
        assert!((t.delta() - 0.016).abs() < 1e-6);
        assert_eq!(t.frame(), 1);
        t.advance(100.0); // clamped to max_delta
        assert!((t.delta() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn time_scale() {
        let mut t = Time::new();
        t.set_scale(0.5);
        t.advance(0.1);
        assert!((t.delta() - 0.05).abs() < 1e-6);
    }

    #[test]
    fn fixed_steps_accumulate() {
        let mut t = Time::new();
        let step = t.fixed_delta();
        t.advance(step * 3.5);
        let mut count = 0;
        while t.step_fixed().is_some() {
            count += 1;
        }
        assert_eq!(count, 3);
        assert!(t.interpolation_alpha() > 0.4 && t.interpolation_alpha() < 0.6);
    }

    #[test]
    fn input_edge_state_resets() {
        let mut input = InputState::new();
        input.press_key(Key::Space);
        assert!(input.key_down(Key::Space));
        assert!(input.key_pressed(Key::Space));
        input.begin_frame();
        assert!(input.key_down(Key::Space), "still held");
        assert!(!input.key_pressed(Key::Space), "edge cleared");
        input.release_key(Key::Space);
        assert!(!input.key_down(Key::Space));
        assert!(input.key_released(Key::Space));
    }

    #[test]
    fn input_axis() {
        let mut input = InputState::new();
        assert_eq!(input.axis(Key::ArrowLeft, Key::ArrowRight), 0.0);
        input.press_key(Key::ArrowRight);
        assert_eq!(input.axis(Key::ArrowLeft, Key::ArrowRight), 1.0);
        input.press_key(Key::ArrowLeft);
        assert_eq!(input.axis(Key::ArrowLeft, Key::ArrowRight), 0.0);
    }

    #[test]
    fn mouse_movement_and_buttons() {
        let mut input = InputState::new();
        input.set_mouse_position(10.0, 5.0);
        assert_eq!(input.mouse.delta, [10.0, 5.0]);
        input.press_mouse(MouseButton::Left);
        assert!(input.mouse.is_down(MouseButton::Left));
        assert!(input.mouse.is_pressed(MouseButton::Left));
        input.begin_frame();
        assert!(input.mouse.is_down(MouseButton::Left));
        assert!(!input.mouse.is_pressed(MouseButton::Left));
        assert_eq!(input.mouse.delta, [0.0, 0.0]);
    }

    #[test]
    fn raw_mouse_motion_accumulates_into_the_delta() {
        let mut input = InputState::new();
        input.set_mouse_position(4.0, 4.0);
        // Raw motion adds to the same frame delta (the captured-cursor path);
        // the absolute position is left alone.
        input.add_mouse_motion(3.0, -2.0);
        assert_eq!(input.mouse.delta, [7.0, 2.0]);
        assert_eq!(input.mouse.position, [4.0, 4.0]);
    }

    #[test]
    fn the_viewport_defaults_and_is_settable() {
        let mut input = InputState::new();
        assert_eq!(input.viewport(), [1280.0, 720.0]);
        input.set_viewport(800, 600);
        assert_eq!(input.viewport(), [800.0, 600.0]);
    }

    #[test]
    fn key_name_round_trip() {
        for key in [
            Key::Letter(0),
            Key::Letter(25),
            Key::Digit(3),
            Key::Space,
            Key::ArrowUp,
            Key::ShiftLeft,
        ] {
            assert_eq!(Key::from_name(&key.name()), Some(key), "{:?}", key.name());
        }
    }

    #[test]
    fn action_map() {
        let mut actions = ActionMap::new();
        actions.bind("Jump", [Key::Space]);
        actions.bind("Left", [Key::Letter(0)]);
        actions.bind("Right", [Key::Letter(3)]);
        let mut input = InputState::new();

        assert!(!actions.is_down("Jump", &input));
        input.press_key(Key::Space);
        assert!(actions.is_down("Jump", &input));
        assert!(actions.is_pressed("Jump", &input));

        input.begin_frame();
        assert!(actions.is_down("Jump", &input), "still held");
        assert!(!actions.is_pressed("Jump", &input), "edge state cleared");

        input.release_key(Key::Space);
        input.press_key(Key::Letter(3)); // Right
        assert_eq!(actions.axis("Left", "Right", &input), 1.0);
        input.press_key(Key::Letter(0)); // Left too
        assert_eq!(actions.axis("Left", "Right", &input), 0.0);
    }

    #[test]
    fn keys_serialize_as_canonical_names() {
        let json = serde_json::to_string(&Key::Letter(22)).unwrap();
        assert_eq!(json, "\"KeyW\"");
        assert_eq!(
            serde_json::to_string(&Key::ShiftLeft).unwrap(),
            "\"ShiftLeft\""
        );
        assert_eq!(
            serde_json::from_str::<Key>("\"Digit3\"").unwrap(),
            Key::Digit(3)
        );
    }

    #[test]
    fn unknown_key_names_are_rejected() {
        let err = serde_json::from_str::<Key>("\"KeyZZ\"").unwrap_err();
        assert!(err.to_string().contains("unknown key name"), "{err}");
    }

    #[test]
    fn mouse_buttons_serialize_as_names() {
        assert_eq!(
            serde_json::to_string(&MouseButton::Middle).unwrap(),
            "\"middle\""
        );
        assert_eq!(
            serde_json::from_str::<MouseButton>("\"right\"").unwrap(),
            MouseButton::Right
        );
        assert!(serde_json::from_str::<MouseButton>("\"thumb\"").is_err());
        assert_eq!(
            MouseButton::from_name(MouseButton::Left.name()),
            Some(MouseButton::Left)
        );
    }

    #[test]
    fn action_map_round_trips_through_json() {
        let mut actions = ActionMap::new();
        actions.bind("Forward", [Key::Letter(22)]);
        actions.bind("Back", [Key::Letter(18)]);

        let json = serde_json::to_string(&actions).unwrap();
        let back: ActionMap = serde_json::from_str(&json).unwrap();
        assert_eq!(back.keys("Forward"), [Key::Letter(22)]);
        assert_eq!(back.keys("Back"), [Key::Letter(18)]);
    }

    #[test]
    fn events_are_visible_next_frame() {
        let mut events: Events<i32> = Events::new();
        events.send(1);
        events.send(2);
        events.update();
        let seen: Vec<i32> = events.read().copied().collect();
        assert_eq!(seen, vec![1, 2]);
        events.update();
        assert!(events.is_empty(), "events drop after being read");
    }
}
