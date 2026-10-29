//! Non-destructive develop: an edit stack applied to a linear-light source.

pub mod composition;
pub mod looks;
pub mod opcodes;
pub mod pipeline;
pub mod source;
pub mod style;

pub use style::{ColorStock, FilmLook, Harmony, HarmonyTemplate, LensFilter, Mono, MonoFilm, Toner};

use serde::{Deserialize, Serialize};

/// Per-colour-band HSL adjustment, each -100..100.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hsl {
    pub hue: f32,
    pub sat: f32,
    pub lum: f32,
}

/// Normalised crop rectangle in the straightened image (0..1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The v1 develop panel. Every field defaults to "no change"; sliders are
/// -100..100 unless noted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Edits {
    /// Stops, -5..5.
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    /// Relative white balance shift from as-shot.
    pub temp: f32,
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
    /// Tone-curve control points (input, output), both 0..1, sorted by input.
    pub curve: Vec<[f32; 2]>,
    /// Red, orange, yellow, green, aqua, blue, purple, magenta.
    pub hsl: [Hsl; 8],
    /// Quarter turns clockwise, 0..3.
    pub rotate: u8,
    pub flip_h: bool,
    /// Straighten angle in degrees, -45..45.
    pub angle: f32,
    pub crop: Option<Crop>,
    /// 0..150.
    pub sharpen: f32,
    /// Pixels at full resolution, 0.5..3.
    pub sharpen_radius: f32,
    /// 0..100.
    pub nr_luma: f32,
    pub nr_color: f32,

    // ---- photographic style ----
    /// Black-and-white conversion: film, lens filter, paper grade, toner.
    pub mono: Mono,
    /// Colour film rendition.
    pub film: FilmLook,
    /// Film grain, 0..100, and its size (1 = 35 mm 400-speed; smaller for
    /// medium/large format).
    pub grain: f32,
    pub grain_size: f32,
    /// Glow, 0..100: soft bloom in black and white, halation in colour.
    pub glow: f32,
    /// Darkroom edge burning, 0..100.
    pub edge_burn: f32,
    pub harmony: Option<Harmony>,
    /// Zone System: the zone a tone was placed in, and N+/N- development
    /// (-2..2) expanding or contracting tones around it.
    pub zone_anchor: Option<u8>,
    pub zone_n: f32,
}

impl Default for Edits {
    fn default() -> Self {
        Edits {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            temp: 0.0,
            tint: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            curve: vec![[0.0, 0.0], [1.0, 1.0]],
            hsl: [Hsl::default(); 8],
            rotate: 0,
            flip_h: false,
            angle: 0.0,
            crop: None,
            sharpen: 0.0,
            sharpen_radius: 1.0,
            nr_luma: 0.0,
            nr_color: 0.0,
            mono: Mono::default(),
            film: FilmLook::default(),
            grain: 0.0,
            grain_size: 1.0,
            glow: 0.0,
            edge_burn: 0.0,
            harmony: None,
            zone_anchor: None,
            zone_n: 0.0,
        }
    }
}

impl Edits {
    pub fn is_default(&self) -> bool {
        *self == Edits::default()
    }

    pub fn from_json(s: Option<&str>) -> Edits {
        s.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderOpts {
    /// Render the whole straightened frame, ignoring the crop (crop tool).
    pub ignore_crop: bool,
    /// Paint clipped highlights red and crushed shadows blue.
    pub clip_warnings: bool,
    /// Ratio of this render's resolution to the full image, for scaling
    /// spatial filters (sharpening/NR radius).
    pub scale: f32,
    /// Show the Zone System map: each tone posterised to its zone (0..X).
    pub zones: bool,
}
