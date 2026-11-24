//! The Creative tab's edit data, kept in the photo's own edit (`RenderSettings::creative`).
//!
//! It is stored in the `.rpd` beside the sliders, curves and masks (the frontend's `creative`
//! object), with serde defaults throughout so a photo edited before the Creative tab existed
//! loads with Bokeh off. The mode and shape names are the frontend's own strings, lower case.

use serde::{Deserialize, Serialize};

/// What the Creative panels hold: Bokeh, Mist and Relight.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Creative {
    pub bokeh: Bokeh,
    pub mist: Mist,
    pub relight: Relight,
}

/// What the Bokeh panel sets. Off by default: a photo that never touched Creative renders
/// exactly as before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bokeh {
    /// Whether it has any strength (the frontend keeps it so: there is no switch): off skips it all.
    pub enabled: bool,
    /// "open" (widen the shot's aperture), "simulate" (another lens) or "manual" (no lens maths).
    pub mode: BokehMode,
    /// The target f-number (open and simulate); 0 is the shot's own.
    pub aperture: f32,
    /// The target focal length in mm (simulate only); 0 is the shot's own.
    pub focal: f32,
    /// 0..100, the manual mode's blur strength.
    pub amount: f32,
    /// The picked focus point: fractions of the upright, uncropped frame.
    pub focus_u: f32,
    pub focus_v: f32,
    /// Focus on the photo's subject (its matte's median distance) instead of the point.
    pub focus_auto: bool,
    /// The in-focus band's extra depth, in control-depth units 0..100 (see `bokeh`'s report: the
    /// control map is log distance, so equal steps look equal near and far, as the masks' band).
    pub focus_range: f32,
    pub shape: BokehShape,
    /// 5..11 (blades only).
    pub blades: u32,
    /// 0..100: 0 a sharp polygon, 100 round (blades only).
    pub blade_rounding: f32,
    /// 0..100: the aperture's cat's-eye squeeze toward the corners.
    pub cats_eye: f32,
    /// 0..100: how much bright, stand-out highlights gain before blurring.
    pub boost: f32,
    /// 0..100: longitudinal colour fringing (purple in front of focus, green behind).
    pub fringe: f32,
}

impl Default for Bokeh {
    fn default() -> Self {
        Bokeh {
            enabled: false,
            mode: BokehMode::Open,
            aperture: 0.0,
            focal: 0.0,
            amount: 0.0,
            focus_u: 0.5,
            focus_v: 0.5,
            focus_auto: true,
            focus_range: 0.0,
            shape: BokehShape::Smooth,
            blades: 7,
            blade_rounding: 50.0,
            cats_eye: 0.0,
            boost: 0.0,
            fringe: 0.0,
        }
    }
}

/// Mist: depth fog (Koschmieder), height fog, sun glow, noise banks and a subject protection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Mist {
    pub enabled: bool,
    /// 0..100: extinction, mapped so 50 gives a visibility a few times the scene's far anchor.
    pub density: f32,
    /// How near the fog comes, in control-depth units 0..100 (0 farthest, 100 nearest): log distance,
    /// the same scale the masks' band and Bokeh's focus range use. 100 reaches the camera.
    pub start: f32,
    /// How far it goes, the same units: 0 has no limit.
    pub far: f32,
    /// 0..100: how softly the fog eases in at Start, in the same log-distance units.
    pub softness: f32,
    /// -100..100: cool to warm fog colour (the brief's temperature-style scale).
    pub temperature: f32,
    /// -100..100: green to magenta.
    pub tint: f32,
    /// 0..100: how strongly the fog pools near the ground (0 = uniform).
    pub height: f32,
    /// 0..100: the height scale's falloff: 0 a very deep layer, 100 a shallow one.
    pub height_falloff: f32,
    /// 0..100: tilts the estimated ground plane (50 = as estimated).
    pub horizon: f32,
    /// 0..100: sun in-scattering (Henyey-Greenstein) into the fog.
    pub glow: f32,
    /// The glow point on the image plane, fractions (0..1 inside the frame; -0.5..1.5 off it).
    pub glow_u: f32,
    pub glow_v: f32,
    /// 0..100: 3D noise banks, only ever adding fog.
    pub texture: f32,
    /// 0 or 1: the noise's seed (deterministic).
    pub seed: f32,
    /// 0..100: how much the BiRefNet subject stays clear of fog.
    pub protect_subject: f32,
}

impl Default for Mist {
    fn default() -> Self {
        Mist {
            enabled: false,
            density: 0.0,
            start: 100.0,
            far: 0.0,
            softness: 15.0,
            temperature: -20.0,
            tint: 0.0,
            height: 0.0,
            height_falloff: 50.0,
            horizon: 50.0,
            glow: 0.0,
            glow_u: 0.5,
            glow_v: 0.2,
            texture: 0.0,
            seed: 1.0,
            protect_subject: 0.0,
        }
    }
}

/// Relight: virtual lights shaped by the scene's own geometry (`relight.rs`). Off until a light is
/// switched on or the ambient is moved: a photo that never touched it costs nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Relight {
    pub enabled: bool,
    /// How the scene's existing light is kept, in stops × 33.3: 0 leaves it as shot, negative dims.
    pub ambient: f32,
    /// 0..100: the photo's own micro-surface tilted into the normals.
    pub relief: f32,
    pub lights: Vec<Light>,
}

impl Default for Relight {
    fn default() -> Self {
        Relight { enabled: false, ambient: 0.0, relief: 0.0, lights: Vec::new() }
    }
}

/// One virtual light. The fields each kind uses are noted; the others keep harmless defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Light {
    /// Stable across edits, for the panel's selection and the Depth Scope's handles.
    pub id: u32,
    pub kind: LightKind,
    pub on: bool,
    /// Point and spot: where it sits on the image plane, fractions of the upright frame; it may sit
    /// off frame (-0.5..1.5).
    pub u: f32,
    pub v: f32,
    /// Point and spot: 0 just in front of the scene, 100 behind it (a backlight).
    pub depth: f32,
    /// Spot only: where its cone points, fractions of the upright frame (the dot on the photo).
    pub aim_u: f32,
    pub aim_v: f32,
    /// Point and spot: the direction the panel (or the spot's cone) faces, as a real 3D angle:
    /// yaw degrees around the vertical (0 = into the scene, 90 = right, 360 wraps) and pitch
    /// degrees above/below the horizon (90 = straight up, −90 = straight down). Absent in older
    /// files, where the aim point above still sets the direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f32>,
    /// 0..100: the light's strength, 0..4 stops over the scene's own light.
    pub intensity: f32,
    /// 2000..12000: the light's colour, as a blackbody's, relative to the photo's own white balance.
    pub kelvin: f32,
    /// -100..100: green to magenta.
    pub tint: f32,
    /// 0..100: how far the light wraps round edges (a large source) and how broad its sheen.
    pub softness: f32,
    /// 0..100: the source's size: its reach in metres (point/spot) and its shadows' penumbra.
    pub size: f32,
    /// 0..100: how much the scene's own surfaces block it.
    pub shadows: f32,
    /// 0..100: how soft those shadows are.
    pub shadow_soft: f32,
    /// Sun only: degrees around the vertical (0 = from the right, 90 = from the top), and above the
    /// horizon, degrees.
    pub azimuth: f32,
    pub elevation: f32,
    /// Point and spot: the cone's half angle, 5..85 degrees (a point light is a panel, so this is how
    /// wide it spreads), and how soft its edge is, 0..100.
    pub cone: f32,
    pub feather: f32,
    /// The modifier picked in the panel's Type list (`softbox`, `rim`, `fill`, ...), so the list
    /// shows what was chosen even when the numbers alone can't tell two modifiers apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier: Option<String>,
}

impl Default for Light {
    fn default() -> Self {
        Light {
            id: 1,
            kind: LightKind::Point,
            on: true,
            u: 0.36,
            v: 0.34,
            depth: 30.0,
            aim_u: 0.5,
            aim_v: 0.5,
            yaw: None,
            pitch: None,
            intensity: 45.0,
            kelvin: 5600.0,
            tint: 0.0,
            softness: 35.0,
            size: 30.0,
            shadows: 0.0,
            shadow_soft: 40.0,
            azimuth: 45.0,
            elevation: 35.0,
            cone: 85.0,
            feather: 45.0,
            modifier: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightKind {
    Point,
    Sun,
    Spot,
    Moon,
}

impl Default for LightKind {
    fn default() -> Self {
        LightKind::Point
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BokehMode {
    Open,
    Simulate,
    Manual,
}

impl Default for BokehMode {
    fn default() -> Self {
        BokehMode::Open
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BokehShape {
    Smooth,
    Blades,
    Bubble,
    Swirl,
    Mirror,
}

impl Default for BokehShape {
    fn default() -> Self {
        BokehShape::Smooth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_without_creative_loads_with_bokeh_off() {
        // The settings a photo edited before this phase sends: no `creative` key at all.
        let s: crate::pipeline::RenderSettings = serde_json::from_str(
            r#"{"exposure":0.4,"contrast":12,"profile":"shinobiColour","crop":{},"gradients":[]}"#,
        )
        .unwrap();
        assert!(!s.creative.bokeh.enabled);
        assert_eq!(s.creative, Creative::default());
    }

    #[test]
    fn the_bokeh_fields_round_trip_in_the_frontends_names() {
        let text = r#"{"bokeh":{"enabled":true,"mode":"simulate","aperture":1.2,"focal":85,
            "amount":40,"focusU":0.42,"focusV":0.61,"focusAuto":false,"focusRange":12,
            "shape":"blades","blades":9,"bladeRounding":20,"catsEye":35,"boost":60,"fringe":15}}"#;
        let c: Creative = serde_json::from_str(text).unwrap();
        assert!(c.bokeh.enabled);
        assert_eq!(c.bokeh.mode, BokehMode::Simulate);
        assert_eq!(c.bokeh.shape, BokehShape::Blades);
        assert_eq!(c.bokeh.blades, 9);
        assert!((c.bokeh.focus_u - 0.42).abs() < 1e-6);
        // An older file with only a few fields gets the rest's defaults.
        let old: Creative = serde_json::from_str(r#"{"bokeh":{"enabled":true}}"#).unwrap();
        assert!(old.bokeh.enabled);
        assert_eq!(old.bokeh.mode, BokehMode::Open);
        assert_eq!(old.bokeh.focus_range, 0.0);
    }
}
