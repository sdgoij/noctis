//! The bang's look (M5): fire and smoke flipbooks, thrown grit, and the mine
//! "tell" — ported from the original's `explosions.js` fx pool.
//!
//! The atlases are **generated** at boot from value noise, exactly as the
//! original does (`fxAtlasPixels`), so the scene gains no binary art and a mod
//! could point at real art later. The engine's billboard pass draws each frame;
//! this module owns the instances and their arithmetic.

use crate::terrain::hash;
use math::Vec3;
use noctis::prelude::*;

/// Fire atlas: `FIRE_COLS`×`FIRE_COLS` cells of `FIRE_CELL` pixels.
const FIRE_COLS: u32 = 4;
const FIRE_CELL: u32 = 32;
/// Smoke atlas: a slower, longer flipbook than the fire.
const SMOKE_COLS: u32 = 6;
const SMOKE_CELL: u32 = 24;
/// The margin each frame keeps inside its cell, in pixels, so a bilinear sample
/// at a frame's edge reads padding rather than the neighbouring frame.
const FX_PAD: u32 = 8;

/// Instances the pool holds; a bang needs four (fire, two smoke, debris).
const FX_CAPACITY: usize = 64;
/// Bodies one debris instance can throw.
const DEBRIS_MAX: usize = 12;

// The bang's tuning (`TUNING.explosions.fx`).
const BLAST_TIME: f32 = 0.5;
const BLAST_SCALE: f32 = 4.2;
const BLAST_LIFT: f32 = 1.1;
const SMOKE_TIME: f32 = 1.2;
const SMOKE_SCALE: f32 = 5.6;
const SMOKE_LIFT: f32 = 2.2;
const DEBRIS_COUNT: usize = 7;
const DEBRIS_TIME: f32 = 0.9;
const DEBRIS_SPEED: f32 = 7.5;
const DEBRIS_UP: f32 = 5.5;
const DEBRIS_GRAVITY: f32 = 22.0;
const DEBRIS_SIZE: f32 = 0.16;

/// Metres within which an armed mine shows its "tell" (`explosions.tell`).
pub const TELL: f32 = 3.0;
/// The tell patch's size, in metres.
pub const TELL_SIZE: f32 = 1.3;
/// The tell patch's tint: faint disturbed earth (`rl.color(46, 40, 32, 120)`).
pub const TELL_TINT: [f32; 4] = [0.18, 0.157, 0.125, 0.47];

/// The generated (or a mod's) atlases, as texture handles.
#[derive(Clone, Copy, Debug)]
pub struct FxTextures {
    /// The fireball flipbook.
    pub fire: AssetHandle<Texture>,
    /// The smoke flipbook.
    pub smoke: AssetHandle<Texture>,
    /// A soft puff, for grit and the mine "tell".
    pub dust: AssetHandle<Texture>,
}

/// Which flipbook an instance shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Kind {
    #[default]
    Fire,
    Smoke,
    /// Grit: `n` bodies on their own streams, no atlas.
    Debris,
}

/// One thrown body's launch state.
#[derive(Clone, Copy, Debug, Default)]
struct Body {
    bx: f32,
    by: f32,
    bz: f32,
    vx: f32,
    vy: f32,
    vz: f32,
}

/// One pooled effect instance.
#[derive(Clone, Copy, Debug, Default)]
struct Instance {
    live: bool,
    kind: Kind,
    x: f32,
    y: f32,
    z: f32,
    age: f32,
    dur: f32,
    scale: f32,
    /// The frame phase it was born with, so two plumes of one bang differ.
    spin: f32,
    /// Debris bodies thrown (`DEBRIS_MAX` at most).
    n: usize,
    bodies: [Body; DEBRIS_MAX],
}

/// A live instance as the renderer sees it: a sprite to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct Visual {
    /// The billboard to draw (texture, atlas frame, tint, size, blend).
    pub billboard: Billboard,
    /// Its world position.
    pub position: Vec3,
}

/// The bang's effect pool: fireballs, smoke plumes and thrown grit.
pub struct Fx {
    slots: Vec<Instance>,
    /// One past the highest slot ever used, so a quiet frame sweeps nothing.
    top: usize,
    textures: FxTextures,
}

impl Fx {
    /// A pool over the generated atlases.
    pub fn new(textures: FxTextures) -> Fx {
        Fx {
            slots: vec![Instance::default(); FX_CAPACITY],
            top: 0,
            textures,
        }
    }

    /// Throw a whole bang at `(x, z)` on `ground`: a fireball, two smoke plumes
    /// and grit (`spawnBlastFx`).
    pub fn blast(&mut self, x: f32, z: f32, ground: f32, seed: f32) {
        self.spawn(
            Kind::Fire,
            x,
            ground + 0.2,
            z,
            seed,
            BLAST_TIME,
            BLAST_SCALE,
            seed,
        );
        // Two plumes: the close one thicker and the far one smaller, slower and
        // longer, which is what gives the column depth rather than one flat puff.
        self.spawn(
            Kind::Smoke,
            x + 0.35,
            ground,
            z - 0.25,
            seed + 1.7,
            SMOKE_TIME,
            SMOKE_SCALE,
            seed * 1.3,
        );
        self.spawn(
            Kind::Smoke,
            x - 0.45,
            ground,
            z + 0.15,
            seed + 3.1,
            SMOKE_TIME * 1.25,
            SMOKE_SCALE * 0.62,
            seed * 0.7,
        );
        self.spawn_debris(x, ground, z, seed);
    }

    /// Advance every live instance, retiring the spent ones (`updateExplosions`).
    pub fn update(&mut self, dt: f32) {
        for i in 0..self.top {
            let slot = &mut self.slots[i];
            if !slot.live {
                continue;
            }
            slot.age += dt;
            if slot.age >= slot.dur {
                slot.live = false;
            }
        }
        while self.top > 0 && !self.slots[self.top - 1].live {
            self.top -= 1;
        }
    }

    /// How many instances are live (the harness's observe seam).
    #[cfg(test)]
    pub fn live(&self) -> usize {
        self.slots[..self.top].iter().filter(|s| s.live).count()
    }

    /// The sprites to draw this frame, in draw order (smoke and grit first, the
    /// fire over them — the original's order).
    pub fn visuals(&self) -> Vec<Visual> {
        let mut out = Vec::new();
        // Alpha-blended first (smoke, grit), then the fire, which is additive.
        for slot in self.slots[..self.top].iter().filter(|s| s.live) {
            match slot.kind {
                Kind::Smoke => self.smoke_visual(slot, &mut out),
                Kind::Debris => self.debris_visuals(slot, &mut out),
                Kind::Fire => {}
            }
        }
        for slot in self.slots[..self.top].iter().filter(|s| s.live) {
            if slot.kind == Kind::Fire {
                self.fire_visual(slot, &mut out);
            }
        }
        out
    }

    fn smoke_visual(&self, slot: &Instance, out: &mut Vec<Visual>) {
        let t = slot.age / slot.dur;
        let fade = 1.0 - t;
        out.push(Visual {
            billboard: Billboard {
                texture: self.textures.smoke,
                size: square(slot.scale * (0.55 + 0.85 * t)),
                color: [0.77, 0.71, 0.65, 0.82 * fade * fade],
                uv: uv_for(SMOKE_COLS, t, slot.spin),
                blend: BillboardBlend::Alpha,
                depth_test: true,
                ..Billboard::default()
            },
            position: Vec3::new(slot.x, slot.y + SMOKE_LIFT * t, slot.z),
        });
    }

    fn fire_visual(&self, slot: &Instance, out: &mut Vec<Visual>) {
        let t = slot.age / slot.dur;
        let fade = 1.0 - t;
        out.push(Visual {
            billboard: Billboard {
                texture: self.textures.fire,
                size: square(slot.scale * (0.62 + 0.55 * t)),
                color: [1.0, 0.84, 0.66, fade * fade],
                uv: uv_for(FIRE_COLS, t, slot.spin),
                blend: BillboardBlend::Additive,
                depth_test: true,
                ..Billboard::default()
            },
            position: Vec3::new(slot.x, slot.y + BLAST_LIFT * t, slot.z),
        });
    }

    fn debris_visuals(&self, slot: &Instance, out: &mut Vec<Visual>) {
        let t = slot.age;
        for body in &slot.bodies[..slot.n] {
            out.push(Visual {
                billboard: Billboard {
                    texture: self.textures.dust,
                    size: square(DEBRIS_SIZE),
                    // Grit is dark, opaque-ish: `rl.color(64, 54, 42, 255)`.
                    color: [0.25, 0.21, 0.16, 1.0],
                    blend: BillboardBlend::Alpha,
                    depth_test: true,
                    ..Billboard::default()
                },
                position: Vec3::new(
                    body.bx + body.vx * t,
                    body.by + body.vy * t - 0.5 * DEBRIS_GRAVITY * t * t,
                    body.bz + body.vz * t,
                ),
            });
        }
    }

    // The spawn's parameters are the instance's whole initial state; a one-use
    // struct would only add indirection.
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        kind: Kind,
        x: f32,
        y: f32,
        z: f32,
        seed: f32,
        dur: f32,
        scale: f32,
        phase: f32,
    ) {
        let Some(at) = self.take() else {
            return;
        };
        let mut slot = Instance {
            live: true,
            kind,
            x,
            y,
            z,
            age: 0.0,
            dur: if dur > 0.0 { dur } else { 0.01 },
            // A little size variation from the seed, as the original does.
            scale: scale * (0.8 + hash(seed) * 0.5),
            spin: phase - phase.floor(),
            ..Instance::default()
        };
        slot.bodies = [Body::default(); DEBRIS_MAX];
        self.slots[at] = slot;
    }

    fn spawn_debris(&mut self, x: f32, ground: f32, z: f32, seed: f32) {
        let count = DEBRIS_COUNT.min(DEBRIS_MAX);
        if count == 0 || DEBRIS_TIME <= 0.0 {
            return;
        }
        let Some(at) = self.take() else {
            return;
        };
        let mut slot = Instance {
            live: true,
            kind: Kind::Debris,
            x,
            y: ground,
            z,
            dur: DEBRIS_TIME,
            n: count,
            ..Instance::default()
        };
        for (i, body) in slot.bodies[..count].iter_mut().enumerate() {
            let i = i as f32;
            let a = hash(seed * 3.1 + i * 1.7) * std::f32::consts::TAU;
            let speed = DEBRIS_SPEED * (0.35 + hash(seed * 5.7 + i * 2.3) * 0.9);
            *body = Body {
                bx: x,
                by: ground + 0.25,
                bz: z,
                vx: a.cos() * speed,
                vz: a.sin() * speed,
                vy: DEBRIS_UP * (0.5 + hash(seed * 7.3 + i * 3.1) * 0.9),
            };
        }
        self.slots[at] = slot;
    }

    fn take(&mut self) -> Option<usize> {
        for i in 0..self.top {
            if !self.slots[i].live {
                return Some(i);
            }
        }
        if self.top < FX_CAPACITY {
            let i = self.top;
            self.top += 1;
            Some(i)
        } else {
            None
        }
    }
}

fn square(size: f32) -> math::Vec2 {
    math::vec2(size, size)
}

/// The atlas UV rect for frame `f` of a `cols`×`cols` grid.
fn uv_for(cols: u32, t: f32, spin: f32) -> [f32; 4] {
    let frames = cols * cols;
    let mut f = if frames <= 1 {
        0.0
    } else {
        (t * (frames - 1) as f32 + spin).floor()
    };
    f = f.clamp(0.0, (frames - 1) as f32);
    let f = f as u32;
    let step = 1.0 / cols as f32;
    let u0 = (f % cols) as f32 * step;
    let v0 = (f / cols) as f32 * step;
    [u0, v0, u0 + step, v0 + step]
}

// ---- the generated atlases --------------------------------------------------

/// The noise tile's edge, in samples.
const FX_TILE: i32 = 32;
const FX_TILE_MASK: i32 = FX_TILE - 1;

/// Value noise on integer mixing rather than a `sin` hash, so the boot cost is
/// arithmetic (the original's trade for the interpreter path).
fn fx_hash(x: i32, y: i32, salt: i32) -> f32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(salt);
    h = (h ^ ((h as u32 >> 13) as i32)).wrapping_mul(1_274_126_177);
    (((h ^ ((h as u32 >> 16) as i32)) as u32) as f32) / 4_294_967_296.0
}

fn fx_noise(x: f32, y: f32, salt: i32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let u = xf * xf * (3.0 - 2.0 * xf);
    let v = yf * yf * (3.0 - 2.0 * yf);
    let (xi, yi) = (xi as i32, yi as i32);
    let a = fx_hash(xi, yi, salt);
    let b = fx_hash(xi + 1, yi, salt);
    let c = fx_hash(xi, yi + 1, salt);
    let d = fx_hash(xi + 1, yi + 1, salt);
    (a + (b - a) * u) * (1.0 - v) + (c + (d - c) * u) * v
}

/// The 32×32 noise field the atlases sample by index. The index order is the
/// original's (`x * TILE + y` on a `y * TILE + x` build), a transposed read that
/// only re-shapes the puff.
fn noise_tile() -> [f32; (FX_TILE * FX_TILE) as usize] {
    let mut out = [0.0f32; (FX_TILE * FX_TILE) as usize];
    for y in 0..FX_TILE {
        for x in 0..FX_TILE {
            let (xf, yf) = (x as f32, y as f32);
            out[(y * FX_TILE + x) as usize] = 0.55 * fx_noise(xf * 0.17, yf * 0.17, 11)
                + 0.30 * fx_noise(xf * 0.34, yf * 0.34, 7)
                + 0.15 * fx_noise(xf * 0.69, yf * 0.69, 5);
        }
    }
    out
}

fn tile_at(tile: &[f32], x: i32, y: i32, ox: i32, oy: i32) -> f32 {
    let xi = ((x + ox) & FX_TILE_MASK) as usize;
    let yi = ((y + oy) & FX_TILE_MASK) as usize;
    tile[xi * FX_TILE as usize + yi]
}

/// The generated fire atlas (`fxAtlasPixels` for `FX_FIRE`): a white-hot core and
/// an orange rim, drawn additively.
pub fn fire_atlas() -> Texture {
    atlas("fx/blast", true, FIRE_COLS, FIRE_CELL)
}

/// The generated smoke atlas: grey, its whole shape from alpha.
pub fn smoke_atlas() -> Texture {
    atlas("fx/smoke", false, SMOKE_COLS, SMOKE_CELL)
}

fn atlas(name: &str, fire: bool, cols: u32, cell: u32) -> Texture {
    let tile = noise_tile();
    let size = cols * cell;
    let mut data = vec![0u8; (size * size * 4) as usize];
    let frames = cols * cols;
    let radius = (cell - FX_PAD * 2) as f32 / cell as f32;
    for f in 0..frames {
        let t = if frames > 1 {
            f as f32 / (frames - 1) as f32
        } else {
            1.0
        };
        let rad = radius
            * if fire {
                0.42 + 0.58 * t
            } else {
                0.30 + 0.70 * t
            };
        let gain = if fire { 2.2 - 0.45 * t } else { 1.8 };
        let bias = if fire { 0.55 } else { 0.45 };
        // Two scrolls of the same tile, so the puff's edge breaks up rather than
        // reading as one soft blob, and neither one repeats over the frame.
        let ox = (f as f32 * 5.3).round() as i32;
        let oy = (f as f32 * 2.7).round() as i32;
        let (col, row) = (f % cols, f / cols);
        for y in 0..cell {
            for x in 0..cell {
                let u = (x as f32 + 0.5) / cell as f32 - 0.5;
                let v = (y as f32 + 0.5) / cell as f32 - 0.5;
                let edge = (u * u + v * v).sqrt() / 0.5 / rad;
                let mut a = 0.0;
                let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
                if edge < 1.0 {
                    let n = 0.62 * tile_at(&tile, (x >> 1) as i32, (y >> 1) as i32, ox, oy)
                        + 0.38 * tile_at(&tile, x as i32, y as i32, oy, ox);
                    let k = ((1.0 - edge) * (n * gain - bias)).clamp(0.0, 1.0);
                    if fire {
                        let heat = 1.0 - edge;
                        a = k;
                        r = 255.0;
                        g = 120.0 + 130.0 * heat;
                        b = 30.0 + 120.0 * heat * heat;
                    } else {
                        a = k * 0.62;
                        r = 120.0;
                        g = 112.0;
                        b = 104.0;
                    }
                }
                let px = col * cell + x;
                let py = row * cell + y;
                let i = ((py * size + px) * 4) as usize;
                data[i] = r.round().clamp(0.0, 255.0) as u8;
                data[i + 1] = g.round().clamp(0.0, 255.0) as u8;
                data[i + 2] = b.round().clamp(0.0, 255.0) as u8;
                data[i + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Texture {
        name: name.to_string(),
        width: size,
        height: size,
        format: TextureFormat::Rgba8Srgb,
        data,
    }
}

/// A soft, slightly-noisy white puff, for grit and the mine "tell" (the original
/// draws both with the weather's cloud puff).
pub fn dust_atlas() -> Texture {
    const N: u32 = 32;
    let tile = noise_tile();
    let mut data = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let u = (x as f32 + 0.5) / N as f32 - 0.5;
            let v = (y as f32 + 0.5) / N as f32 - 0.5;
            let edge = (u * u + v * v).sqrt() / 0.5;
            let n = tile_at(&tile, x as i32, y as i32, 0, 0);
            let a = ((1.0 - edge).clamp(0.0, 1.0).powi(2) * (0.55 + 0.45 * n)).clamp(0.0, 1.0);
            let i = ((y * N + x) * 4) as usize;
            data[i] = 255;
            data[i + 1] = 255;
            data[i + 2] = 255;
            data[i + 3] = (a * 255.0).round() as u8;
        }
    }
    Texture {
        name: "fx/dust".to_string(),
        width: N,
        height: N,
        format: TextureFormat::Rgba8Srgb,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_atlas_is_the_grid_size() {
        let atlas = fire_atlas();
        assert_eq!(atlas.width, FIRE_COLS * FIRE_CELL);
        assert_eq!(atlas.height, FIRE_COLS * FIRE_CELL);
        assert_eq!(atlas.data.len(), (atlas.width * atlas.height * 4) as usize);
        let smoke = smoke_atlas();
        assert_eq!(smoke.width, SMOKE_COLS * SMOKE_CELL);
    }

    #[test]
    fn the_atlas_is_not_blank() {
        let atlas = fire_atlas();
        let opaque = atlas.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0);
        assert!(opaque, "the fire atlas must carry some coverage");
    }

    #[test]
    fn the_frames_pad_their_cells() {
        // The four corners of every frame's cell are clear, so a bilinear sample at
        // the edge reads padding, not the neighbouring frame.
        let atlas = fire_atlas();
        let size = atlas.width;
        for f in 0..(FIRE_COLS * FIRE_COLS) {
            let (col, row) = (f % FIRE_COLS, f / FIRE_COLS);
            for (dx, dy) in [
                (0, 0),
                (FIRE_CELL - 1, 0),
                (0, FIRE_CELL - 1),
                (FIRE_CELL - 1, FIRE_CELL - 1),
            ] {
                let x = col * FIRE_CELL + dx;
                let y = row * FIRE_CELL + dy;
                let a = atlas.data[((y * size + x) * 4 + 3) as usize];
                assert_eq!(a, 0, "frame {f} corner ({dx},{dy}) must be clear");
            }
        }
    }

    #[test]
    fn frame_uv_steps_across_the_grid() {
        // Frame 0 is the top-left cell; the last frame the bottom-right.
        let first = uv_for(4, 0.0, 0.0);
        assert_eq!(first, [0.0, 0.0, 0.25, 0.25]);
        let last = uv_for(4, 1.0, 0.0);
        assert_eq!(last, [0.75, 0.75, 1.0, 1.0]);
        // A mid-life frame sits somewhere between, and never outside the grid.
        let mid = uv_for(4, 0.5, 0.0);
        assert!(mid[0] >= 0.0 && mid[2] <= 1.0);
    }

    #[test]
    fn a_bang_spawns_fire_smoke_and_debris_and_retires_them() {
        let handles = FxTextures {
            fire: AssetHandle::default(),
            smoke: AssetHandle::default(),
            dust: AssetHandle::default(),
        };
        let mut fx = Fx::new(handles);
        assert_eq!(fx.live(), 0);

        fx.blast(2.0, 3.0, 0.5, 1.5);
        assert_eq!(fx.live(), 4, "fire, two smoke, one debris batch");

        // The visuals are the fireball, two plumes and the grit.
        let visuals = fx.visuals();
        assert_eq!(visuals.len(), 1 + 2 + DEBRIS_COUNT);
        assert!(
            visuals
                .iter()
                .any(|v| v.billboard.blend == BillboardBlend::Additive),
            "the fireball is additive"
        );
        assert!(
            visuals.iter().all(|v| v.position.y.is_finite()),
            "every sprite is placed"
        );

        // Slower than the longest effect and the pool is empty.
        fx.update(2.0);
        assert_eq!(fx.live(), 0);
        assert!(fx.visuals().is_empty());
    }

    #[test]
    fn grit_falls_under_its_own_gravity() {
        let handles = FxTextures {
            fire: AssetHandle::default(),
            smoke: AssetHandle::default(),
            dust: AssetHandle::default(),
        };
        let mut fx = Fx::new(handles);
        fx.spawn_debris(0.0, 1.0, 0.0, 2.0);
        // The slowest body is the one that falls furthest, so the field's floor
        // (its minimum height) is what comes down.
        let floor = |fx: &Fx| {
            fx.visuals()
                .iter()
                .map(|v| v.position.y)
                .fold(f32::INFINITY, f32::min)
        };
        let high = floor(&fx);
        fx.update(0.5);
        let low = floor(&fx);
        assert!(low < high, "the grit must come down: {high} -> {low}");
    }

    #[test]
    fn the_pool_is_bounded() {
        let handles = FxTextures {
            fire: AssetHandle::default(),
            smoke: AssetHandle::default(),
            dust: AssetHandle::default(),
        };
        let mut fx = Fx::new(handles);
        // Many bangs at once cannot grow the pool past its capacity.
        for i in 0..100 {
            fx.blast(i as f32, 0.0, 0.0, i as f32);
        }
        assert!(fx.live() <= FX_CAPACITY);
    }
}
