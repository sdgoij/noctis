/*!
 * Coloring and palette logic
 */

use std::cell::Cell;

/// A simple RGB color
pub type Color = [u8; 3];

/// A Palette is just 4 colors (NES inspired)
pub type Palette = [Color; 4];

thread_local! {
    /// A tiny SplitMix64 for palette randomization. The original used `rand`,
    /// whose `getrandom` backend has no default on `wasm32-unknown-unknown`; this
    /// needs nothing from the target.
    static RNG: Cell<u64> = const { Cell::new(0x9E37_79B9_7F4A_7C15) };
}

fn random_u8() -> u8 {
    RNG.with(|state| {
        let mut z = state.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        state.set(z);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) as u8
    })
}

/// generate a random palette
pub fn random_palette() -> Palette {
    let mut palette = [[0; 3]; 4];

    for color in &mut palette {
        *color = [random_u8(), random_u8(), random_u8()];
    }

    // sort colors by brightness (our tilemap is)
    // https://stackoverflow.com/questions/20213421/sorting-color-selections-on-brightness
    palette.sort_by_key(|[r, g, b]| 299 * *r as u32 + 587 * *g as u32 + 114 * *b as u32);

    palette
}
