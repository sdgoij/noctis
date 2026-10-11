// The `env` module Slag's wasm build imports its clock from.
//
// Slag (the scripting engine) compiled to wasm imports two host functions from a
// module named `env`:
//
//   env.slag_host_now_ms            -> wall-clock milliseconds
//   env.slag_host_now_monotonic_ms  -> a monotonic millisecond clock
//
// wasm-bindgen cannot resolve a bare `env` specifier, so `index.html` maps it
// here with an import map (`{"imports": {"env": "./env.js"}}`). The names are
// exactly the wasm's imports; a namespace import (`import * as …`) picks up every
// export.
export const slag_host_now_ms = () => Date.now();
export const slag_host_now_monotonic_ms = () => performance.now();
