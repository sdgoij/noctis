//! A native reachability validator for the level editor.
//!
//! The editor extension calls this as `noctis.call("reverie.validate", { level,
//! budget })` — the project's [`Plugin::extension_functions`][plugin] table wires
//! it up in `src/lib.rs`. It exists because the extension's JavaScript cannot do
//! this well twice over: the game's integrator is transcribed there (so it can
//! drift from `src/game/`), and its search is too narrow.
//!
//! Here the search calls the game's own [`LevelDefinition::parse`] and
//! [`Player::update`], so a verdict is a verdict about the real game. It is a
//! macro-action search: a **node** is a place the player came to rest (the pixel
//! position and the horizontal speed it arrived with), and an **arc** is one input
//! program simulated through the real integrator until it lands. The programs cover
//! what the JavaScript arc model could not — a direction change or a stop *in
//! flight*, which is what a tight maze needs — and the landing keeps its pixel
//! position instead of being rounded to a tile, so a search does not lose the
//! precision a later jump depends on.
//!
//! [plugin]: ecs::app::Plugin::extension_functions

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::game::{HITBOX_HEIGHT, HITBOX_WIDTH, Input, LevelDefinition, Player, TILE_SIZE};

/// The game's fixed step (`src/game/mod.rs` runs the real delta; at 60 Hz it is
/// this, and a fixed step makes a verdict reproducible).
const STEP: f32 = 1.0 / 60.0;
/// How far below the level floor a fall counts as a death (`src/game/mod.rs`).
const DEATH_PLANE: f32 = 70.0;
/// How long one arc may run before it is abandoned (1.6 s at 60 Hz).
const MAX_FRAMES: usize = 96;
/// The most nodes one call explores before it reports itself truncated.
const DEFAULT_BUDGET: usize = 60_000;
/// The route keeps every `SAMPLE`-th frame.
const SAMPLE: usize = 3;
/// Where in a landing's footprint an arc may take off from: a body may overhang a
/// ledge by its own width, and which end it stands on decides a marginal jump.
const TAKEOFFS: &[f32] = &[-8.0, 0.0, 8.0];

#[derive(Deserialize)]
struct Request {
    /// The level's text, as the game's own parser reads it.
    level: String,
    /// The most nodes to explore (clamped).
    #[serde(default)]
    budget: Option<usize>,
}

#[derive(Default, Serialize)]
struct Route {
    xs: Vec<f32>,
    ys: Vec<f32>,
}

#[derive(Serialize)]
struct Report {
    /// Whether the goal was reached.
    goal: bool,
    /// Whether the budget ran out before the walk finished.
    truncated: bool,
    /// The number of nodes explored — the cost, for the panel and the log.
    states: usize,
    /// The cells the body occupied, row-major.
    reached: Vec<u32>,
    /// The cells of the coins the body touched.
    coins: Vec<u32>,
    /// The route to the goal: the body's top-left corner per sampled frame.
    route: Route,
}

/// One input program: the direction held from takeoff, how long jump is held, and
/// an optional mid-air change of direction.
#[derive(Clone, Copy)]
struct Policy {
    /// The direction held from takeoff: -1, 0 or 1.
    dir: i8,
    /// Frames the jump key is held; `-1` never jumps (a walk or a walk-off).
    hold: i8,
    /// The frame the direction changes (`-1` never).
    switch_at: i16,
    /// The direction after the change.
    switch_dir: i8,
}

/// Every program the search tries from every takeoff point.
///
/// The jump holds sample the climb from a tap to full height; the mid-air changes
/// are what make a reversal or a mid-air stop reachable at all — the thing the
/// JavaScript model could not express, and the reason `level3` read as no route.
fn policies() -> Vec<Policy> {
    let mut out = Vec::new();
    for dir in [-1i8, 0, 1] {
        // Walks and walk-offs: no jump. Standing still is not a move, so it has no
        // walk of its own; the walk with a mid-air stop or reversal is the one a
        // narrow ledge needs.
        if dir != 0 {
            out.push(Policy {
                dir,
                hold: -1,
                switch_at: -1,
                switch_dir: 0,
            });
            for (switch_at, switch_dir) in [(10i16, 0i8), (20, -dir)] {
                out.push(Policy {
                    dir,
                    hold: -1,
                    switch_at,
                    switch_dir,
                });
            }
        }
        // Jumps at five holds, each with and without a mid-air change.
        for hold in [0i8, 6, 12, 18, 26] {
            out.push(Policy {
                dir,
                hold,
                switch_at: -1,
                switch_dir: 0,
            });
            for (switch_at, switch_dir) in [(8i16, 0i8), (20, -dir)] {
                out.push(Policy {
                    dir,
                    hold,
                    switch_at,
                    switch_dir,
                });
            }
        }
    }
    out
}

/// A place the player came to rest, and the arc that got it there.
struct Node {
    /// The player at rest (its `x`, `y` and `vx` are what matter).
    player: Player,
    /// The state the arc took off from, kept so the route can be replayed exactly.
    start: Player,
    /// The policy the arc ran.
    policy: u16,
    /// The node it came from (the root points at itself).
    parent: u32,
}

/// The `Plugin::extension_functions` entries.
///
/// `validate` runs one search to completion (the CLI has no frames to poll on);
/// `start`/`poll` do the same search on a worker thread, so a level that takes a
/// second to check does not freeze the frame it was edited in. A new `start`
/// cancels the one before it.
pub(crate) fn validate(args: &str) -> std::result::Result<String, String> {
    validate_level(args, None).map_err(|error| format!("{error:#}"))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn start(args: &str) -> std::result::Result<String, String> {
    jobs::start(args)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn poll(_args: &str) -> std::result::Result<String, String> {
    jobs::poll()
}

fn validate_level(args: &str, cancel: Option<&AtomicBool>) -> Result<String> {
    let request: Request = serde_json::from_str(args).context("bad arguments")?;
    let level = LevelDefinition::parse(&request.level).context("the game rejects this level")?;
    let budget = request.budget.unwrap_or(DEFAULT_BUDGET).clamp(1, 20_000_000);
    serde_json::to_string(&search(&level, budget, cancel)).context("the report does not serialize")
}

/// One in-flight background search.
///
/// The editor extensions are stateless function pointers, so the job lives in a
/// process-wide slot: one validator, one editor.
#[cfg(not(target_arch = "wasm32"))]
mod jobs {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, TryRecvError};
    use std::sync::{Arc, Mutex};

    struct Job {
        rx: Receiver<std::result::Result<String, String>>,
        cancel: Arc<AtomicBool>,
    }

    static RUNNING: Mutex<Option<Job>> = Mutex::new(None);

    pub fn start(args: &str) -> Result<String, String> {
        let args = args.to_owned();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let outcome = super::validate_level(&args, Some(&flag))
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(outcome);
        });
        let mut running = RUNNING
            .lock()
            .map_err(|_| "the validator lock is poisoned".to_string())?;
        // The previous edit's search has nothing to say about this grid.
        if let Some(old) = running.take() {
            old.cancel.store(true, Ordering::Relaxed);
        }
        *running = Some(Job { rx, cancel });
        Ok("started".to_string())
    }

    pub fn poll() -> Result<String, String> {
        let mut running = RUNNING
            .lock()
            .map_err(|_| "the validator lock is poisoned".to_string())?;
        let Some(job) = running.as_mut() else {
            return Ok(serde_json::json!({ "running": false, "error": "nothing to poll" }).to_string());
        };
        match job.rx.try_recv() {
            // The report is already JSON text; splice it in.
            Ok(Ok(report)) => {
                *running = None;
                Ok(format!(r#"{{"running":false,"report":{report}}}"#))
            }
            Ok(Err(error)) => {
                *running = None;
                Ok(serde_json::json!({ "running": false, "error": error }).to_string())
            }
            Err(TryRecvError::Empty) => Ok(r#"{"running":true}"#.to_string()),
            Err(TryRecvError::Disconnected) => {
                *running = None;
                Ok(serde_json::json!({ "running": false, "error": "the validator stopped" }).to_string())
            }
        }
    }
}

/// Walk the level from the spawn, one landing at a time.
fn search(level: &LevelDefinition, budget: usize, cancel: Option<&AtomicBool>) -> Report {
    let width = level.width;
    let height = level.height;
    let cells = width * height;
    let max_x = width as f32 * TILE_SIZE as f32 - HITBOX_WIDTH;
    let death = height as f32 * TILE_SIZE as f32 + DEATH_PLANE;

    // One array index per lookup: the hot loop must not look a coin up by cell.
    let mut coin_at = vec![-1i32; cells];
    for (index, &(x, y)) in level.coins.iter().enumerate() {
        coin_at[y * width + x] = index as i32;
    }
    let goal_cell = (level.goal.1 * width + level.goal.0) as i32;

    let mut reached = vec![false; cells];
    let mut coin_hit = vec![false; level.coins.len()];
    let mut coins_left = level.coins.len();
    let mut goal = false;
    let mut goal_node = None;

    // The spawn is airborne: the game drops the player there. The first arcs are
    // the fall, with whatever air control each policy asks for.
    let spawn = Player::new(
        (level.spawn.0 * TILE_SIZE) as f32,
        (level.spawn.1 * TILE_SIZE) as f32,
    );
    let mut nodes: Vec<Node> = Vec::with_capacity(1024);
    nodes.push(Node {
        player: spawn.clone(),
        start: spawn.clone(),
        policy: 0,
        parent: 0,
    });
    let mut seen: HashMap<u64, u32> = HashMap::with_capacity(1024);
    seen.insert(node_key(&spawn), 0);
    let mut head = 0usize;
    let mut truncated = false;
    let programs = policies();
    let mut samples = Vec::new();

    'walk: while head < nodes.len() {
        if goal && coins_left == 0 {
            break;
        }
        if nodes.len() >= budget {
            truncated = true;
            break;
        }
        if let Some(flag) = cancel
            && head.is_multiple_of(256)
            && flag.load(Ordering::Relaxed)
        {
            truncated = true;
            break;
        }
        // Copy what the expansion needs, so the loop can push to `nodes`.
        let (landing, grounded) = {
            let node = &nodes[head];
            (node.player.clone(), node.player.grounded)
        };
        head += 1;

        // A landing may take off from anywhere along its footprint; a body in the
        // air only from where it is.
        let offsets: &[f32] = if grounded { TAKEOFFS } else { &[0.0] };
        for &offset in offsets {
            let takeoff_x = (landing.x + offset).clamp(0.0, max_x);
            if grounded && !standing(level, takeoff_x, landing.y) {
                continue;
            }
            let mut start = landing.clone();
            start.x = takeoff_x;
            for (index, policy) in programs.iter().enumerate() {
                samples.clear();
                let outcome = simulate(
                    level,
                    &start,
                    policy,
                    max_x,
                    death,
                    &coin_at,
                    goal_cell,
                    &mut reached,
                    &mut coin_hit,
                    &mut coins_left,
                    &mut goal,
                    &mut samples,
                );
                let Some(rested) = outcome else {
                    continue;
                };
                let state = node_key(&rested);
                let child = match seen.get(&state) {
                    Some(&known) => known,
                    None => {
                        let child = nodes.len() as u32;
                        nodes.push(Node {
                            player: rested,
                            start: start.clone(),
                            policy: index as u16,
                            parent: (head - 1) as u32,
                        });
                        seen.insert(state, child);
                        if nodes.len() >= budget {
                            truncated = true;
                            break 'walk;
                        }
                        child
                    }
                };
                if goal && goal_node.is_none() {
                    goal_node = Some(child);
                }
            }
        }
    }

    #[cfg(test)]
    {
        eprintln!(
            "nodes={} truncated={} goal={} coins_left={}",
            nodes.len(),
            truncated,
            goal,
            coins_left
        );
    }

    let mut route = Route::default();
    if let Some(mut at) = goal_node {
        let mut chain = Vec::new();
        loop {
            chain.push(at);
            let node = &nodes[at as usize];
            if node.parent == at {
                break;
            }
            at = node.parent;
        }
        chain.reverse();
        for (step, &index) in chain.iter().enumerate() {
            let node = &nodes[index as usize];
            if index == 0 {
                // The spawn itself: no arc to replay.
                route.xs.push(node.player.x);
                route.ys.push(node.player.y);
                continue;
            }
            samples.clear();
            simulate(
                level,
                &node.start,
                &programs[node.policy as usize],
                max_x,
                death,
                &coin_at,
                goal_cell,
                &mut reached,
                &mut coin_hit,
                &mut coins_left,
                &mut goal,
                &mut samples,
            );
            for (frame, &(x, y)) in samples.iter().enumerate() {
                if frame % SAMPLE == 0 || frame + 1 == samples.len() || step + 1 == chain.len() {
                    route.xs.push(x);
                    route.ys.push(y);
                }
            }
        }
    }

    Report {
        goal,
        truncated,
        states: nodes.len(),
        reached: (0..cells as u32).filter(|cell| reached[*cell as usize]).collect(),
        coins: level
            .coins
            .iter()
            .enumerate()
            .filter(|(index, _)| coin_hit[*index])
            .map(|(_, &(x, y))| (y * width + x) as u32)
            .collect(),
        route,
    }
}

/// Run one policy from `start` until it lands, marking what the body covered on the
/// way. Returns the resting player, or `None` when the arc fell out of the level.
#[allow(clippy::too_many_arguments)]
fn simulate(
    level: &LevelDefinition,
    start: &Player,
    policy: &Policy,
    max_x: f32,
    death: f32,
    coin_at: &[i32],
    goal_cell: i32,
    reached: &mut [bool],
    coin_hit: &mut [bool],
    coins_left: &mut usize,
    goal: &mut bool,
    samples: &mut Vec<(f32, f32)>,
) -> Option<Player> {
    let mut player = start.clone();
    let mut dir = policy.dir;
    let jumping = policy.hold >= 0;
    let mut left_ground = !player.grounded;
    for frame in 0..MAX_FRAMES {
        if policy.switch_at >= 0 && frame as i16 == policy.switch_at {
            dir = policy.switch_dir;
        }
        let input = Input {
            left: dir < 0,
            right: dir > 0,
            jump: jumping && frame as i8 <= policy.hold,
            jump_pressed: jumping && frame == 0,
            ..Input::default()
        };
        player.update(&input, STEP, &level.blocks, max_x);
        if player.y > death {
            return None;
        }
        cover(
            &player,
            level,
            coin_at,
            goal_cell,
            reached,
            coin_hit,
            coins_left,
            goal,
        );
        if frame % SAMPLE == 0 {
            samples.push((player.x, player.y));
        }
        if !player.grounded {
            left_ground = true;
        } else if left_ground {
            samples.push((player.x, player.y));
            return Some(player);
        }
    }
    // A walk on flat ground never leaves it: the arc ends where the walk got to.
    if player.grounded && !left_ground {
        samples.push((player.x, player.y));
        return Some(player);
    }
    None
}

/// Mark every tile the body overlaps, and report whether the goal is one of them.
///
/// The game checks the same overlap after each frame (`src/game/mod.rs`), so this
/// is the game's own collision footprint, not a guess at it.
#[allow(clippy::too_many_arguments)]
fn cover(
    player: &Player,
    level: &LevelDefinition,
    coin_at: &[i32],
    goal_cell: i32,
    reached: &mut [bool],
    coin_hit: &mut [bool],
    coins_left: &mut usize,
    goal: &mut bool,
) {
    let tile = TILE_SIZE as f32;
    let left = (player.x / tile).floor() as i32;
    let right = ((player.x + HITBOX_WIDTH - 0.01) / tile).floor() as i32;
    let top = (player.y / tile).floor() as i32;
    let bottom = ((player.y + HITBOX_HEIGHT - 0.01) / tile).floor() as i32;
    for ty in top.max(0)..=bottom {
        if ty as usize >= level.height {
            continue;
        }
        for tx in left.max(0)..=right {
            if tx as usize >= level.width {
                continue;
            }
            let cell = ty as usize * level.width + tx as usize;
            reached[cell] = true;
            let coin = coin_at[cell];
            if coin >= 0 && !coin_hit[coin as usize] {
                coin_hit[coin as usize] = true;
                *coins_left -= 1;
            }
            if cell as i32 == goal_cell {
                *goal = true;
            }
        }
    }
}

/// Whether a body at `(x, y)` is clear of tiles and standing on one — the same test
/// the JavaScript model used, and the reason the takeoff offsets can overhang.
fn standing(level: &LevelDefinition, x: f32, y: f32) -> bool {
    let tile = TILE_SIZE as f32;
    let row = (y / tile).floor() as i32;
    let left = (x / tile).floor() as i32;
    let right = ((x + HITBOX_WIDTH - 0.01) / tile).floor() as i32;
    let mut supported = false;
    for tx in left..=right {
        if solid(level, tx, row) {
            return false;
        }
        if solid(level, tx, row + 1) {
            supported = true;
        }
    }
    supported
}

fn solid(level: &LevelDefinition, tx: i32, ty: i32) -> bool {
    if tx < 0 || ty < 0 || tx as usize >= level.width || ty as usize >= level.height {
        return false;
    }
    level.blocks[ty as usize][tx as usize]
}

/// The deduplication key: the landing position to 2 px, its row, and the horizontal
/// speed it arrived with (bucketed). Two arcs that rest at the same place with the
/// same momentum lead to the same places, so one of them is enough.
fn node_key(player: &Player) -> u64 {
    let x = (player.x.clamp(0.0, 8191.0) / 2.0) as u64;
    let row = (player.y.clamp(0.0, 2047.0) / 16.0) as u64;
    let vx = ((player.vx + 160.0) / 40.0).clamp(0.0, 7.0) as u64;
    x | (row << 13) | (vx << 24) | ((player.grounded as u64) << 27)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Report {
        let level = LevelDefinition::parse(text).expect("the level parses");
        search(&level, DEFAULT_BUDGET, None)
    }

    #[test]
    fn the_first_two_levels_are_playable() {
        for (name, text) in [
            ("level1", include_str!("../levels/level1.txt")),
            ("level2", include_str!("../levels/level2.txt")),
        ] {
            let started = std::time::Instant::now();
            let report = run(text);
            eprintln!("{name}: {} nodes in {:?}", report.states, started.elapsed());
            assert!(report.goal, "{name} should be playable");
            assert!(!report.truncated, "{name} fits the budget");
            assert!(!report.route.xs.is_empty(), "{name} has a route");
        }
    }

    /// `level3` is the case the JavaScript arc sweep could not solve: its maze needs
    /// a mid-air reversal. The programs here have one.
    #[test]
    fn the_maze_level_is_playable() {
        let started = std::time::Instant::now();
        let report = run(include_str!("../levels/level3.txt"));
        eprintln!("level3: {} nodes in {:?}", report.states, started.elapsed());
        assert!(
            report.goal,
            "level3 should be playable ({} nodes, truncated {})",
            report.states, report.truncated
        );
        assert!(!report.truncated, "level3 fits the budget");
    }

    #[test]
    fn a_wall_across_the_level_is_found() {
        let mut level = LevelDefinition::parse(include_str!("../levels/level1.txt")).expect("parse");
        for row in level.blocks.iter_mut() {
            row[20] = true;
        }
        let report = search(&level, DEFAULT_BUDGET, None);
        assert!(!report.goal, "a full-height wall seals the goal off");
        assert!(!report.truncated, "the walk finishes");
    }

    #[test]
    fn a_level_the_game_rejects_is_an_error() {
        let error = validate_level(
            r#"{"level": "name: x\nmusic: y\nbackground: z\npalette: #000000, #000000, #000000, #000000\n\nGGGG\n"}"#,
            None,
        )
        .expect_err("no spawn");
        assert!(format!("{error:#}").contains("no spawn"), "{error:#}");
    }

    /// The threaded path the editor uses: `start` hands the search to a worker and
    /// `poll` collects the report.
    #[test]
    fn the_threaded_job_delivers_its_report() {
        let level =
            "name: t\nmusic: x\nbackground: y\npalette: #000000, #000000, #000000, #000000\n\nS.F\nGGG\n";
        let args = serde_json::json!({ "level": level }).to_string();
        start(&args).expect("the job starts");
        for _ in 0..4000 {
            let reply = poll("").expect("poll");
            let message: serde_json::Value = serde_json::from_str(&reply).expect("json");
            if message["running"].as_bool() == Some(false) {
                assert_eq!(message["report"]["goal"].as_bool(), Some(true), "{reply}");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the job never finished");
    }
}
