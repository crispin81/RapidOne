//! The colour tools after the Basic panel: Fine tuning (Lightroom's
//! Calibration), Mixer (HSL), Curves, Grade (Color Grading) and Vignette.
//!
//! Each is our own model, with a few numbers per tool (`ToolTuning`) fitted
//! by measuring Lightroom's response (`examples/toolmatch.rs`), so the same
//! slider values give the same look.
//!
//! Where they run:
//! - Fine tuning works on scene light, like a camera calibration: it moves
//!   the red, green and blue primaries and tints the shadows.
//! - The rest work on the rendered (display-linear Rec.2020) image after the
//!   tone curve: Mixer in OKLab, Curves on gamma-encoded values, Grade as
//!   colour pushes weighted by brightness, Vignette by position.

use crate::dngcolour::Curve;
use crate::pipeline::Oklab;

pub type Mat3 = [[f32; 3]; 3];

/// Mixer band centres on the (display RGB) hue circle, degrees: red,
/// orange, yellow, green, aqua, blue, purple, magenta - as in Lightroom.
pub(crate) const BAND_CENTRES: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];

/// Rec.2020 luminance weights.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// A tone range's colour push and luminance (Lightroom's Color Grading).
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GradeRange {
    /// 0..360, Lightroom's hue (0 red, 120 green, 240 blue).
    pub hue: f32,
    /// 0..100.
    pub sat: f32,
    /// -100..100.
    pub lum: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GradeSettings {
    pub shadows: GradeRange,
    pub mids: GradeRange,
    pub highs: GradeRange,
    pub global: GradeRange,
    /// 0..100 (default 50).
    pub blending: f32,
    /// -100..100.
    pub balance: f32,
}

impl Default for GradeSettings {
    fn default() -> Self {
        Self { shadows: Default::default(), mids: Default::default(), highs: Default::default(), global: Default::default(), blending: 50.0, balance: 0.0 }
    }
}

impl GradeSettings {
    fn is_neutral(&self) -> bool {
        [self.shadows, self.mids, self.highs, self.global].iter().all(|r| r.sat == 0.0 && r.lum == 0.0)
    }
}

/// The tools' slider values, as the panels hold them.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolSettings {
    pub cal_shadow_tint: f32,
    pub cal_red_hue: f32,
    pub cal_red_sat: f32,
    pub cal_green_hue: f32,
    pub cal_green_sat: f32,
    pub cal_blue_hue: f32,
    pub cal_blue_sat: f32,
    /// Per band (red .. magenta): hue, saturation, luminance, each -100..100.
    pub mixer: [[f32; 3]; 8],
    /// Point curves on 0..255 (Lightroom's units): RapidOne's Luminosity
    /// curve (brightness only, colour kept), the RGB curve, then red, green
    /// and blue. Empty = straight.
    pub curve_luma: Vec<[f32; 2]>,
    pub curve_rgb: Vec<[f32; 2]>,
    pub curve_red: Vec<[f32; 2]>,
    pub curve_green: Vec<[f32; 2]>,
    pub curve_blue: Vec<[f32; 2]>,
    /// Lightroom's parametric (region) curve, -100..100 each: Shadows,
    /// Darks, Lights, Highlights; and its splits, 0..100 (Lightroom's 25 /
    /// 50 / 75 by default).
    pub param_shadows: f32,
    pub param_darks: f32,
    pub param_lights: f32,
    pub param_highlights: f32,
    pub param_shadow_split: f32,
    pub param_mid_split: f32,
    pub param_high_split: f32,
    pub grade: GradeSettings,
    /// -100..100, Lightroom's post-crop vignette amount (highlight priority):
    /// the legacy vignette, an oval shaped by the Midpoint, Feather,
    /// Roundness and Highlights below (Lightroom's names, as in the XMP).
    pub vignette: f32,
    /// -100..100, RapidOne's adaptive vignette: darkens around the subject's
    /// outline (subject.rs) instead of an oval. Its own setting; both can be used.
    pub adaptive_vignette: f32,
    /// The classic vignette's shape, as Lightroom's sliders: Midpoint and
    /// Feather 0..100 (50 = our measured default), Roundness -100..100.
    pub vignette_midpoint: f32,
    pub vignette_feather: f32,
    pub vignette_roundness: f32,
    /// 0..100, Lightroom's vignette Highlights: bright parts (lights, a white
    /// dress) are let through a darkening vignette.
    #[serde(default)]
    pub vignette_highlights: f32,
    /// The subject-aware vignette's weights for this photo (from its subject
    /// mask), filled in by the renderer; None = the oval.
    #[serde(skip)]
    pub vignette_map: Option<std::sync::Arc<crate::subject::VignetteMap>>,
    /// -100..100 each: local contrast at a small (Texture) and large
    /// (Clarity) radius, and haze removal (Dehaze).
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    /// Detail (`detail.rs`): sharpening 0..150, luminance noise reduction
    /// 0..100 and film grain 0..100.
    pub sharpen: f32,
    pub noise: f32,
    pub grain: f32,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            cal_shadow_tint: 0.0,
            cal_red_hue: 0.0,
            cal_red_sat: 0.0,
            cal_green_hue: 0.0,
            cal_green_sat: 0.0,
            cal_blue_hue: 0.0,
            cal_blue_sat: 0.0,
            mixer: [[0.0; 3]; 8],
            curve_luma: Vec::new(),
            curve_rgb: Vec::new(),
            curve_red: Vec::new(),
            curve_green: Vec::new(),
            curve_blue: Vec::new(),
            param_shadows: 0.0,
            param_darks: 0.0,
            param_lights: 0.0,
            param_highlights: 0.0,
            param_shadow_split: 25.0,
            param_mid_split: 50.0,
            param_high_split: 75.0,
            grade: GradeSettings::default(),
            vignette: 0.0,
            adaptive_vignette: 0.0,
            vignette_midpoint: 50.0,
            vignette_feather: 50.0,
            vignette_roundness: 0.0,
            vignette_highlights: 0.0,
            vignette_map: None,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            sharpen: 0.0,
            noise: 0.0,
            grain: 0.0,
        }
    }
}

/// The fitted numbers that shape each tool (see `examples/toolmatch.rs`).
#[derive(Clone, Copy, Debug)]
pub struct ToolTuning {
    /// Fine tuning: how far a primary moves towards its neighbour at hue
    /// Â±100 (fraction of the neighbour mixed in), per primary R, G, B.
    pub cal_hue: [f32; 3],
    /// How much of the third primary a hue turn takes out (0 = mix only).
    pub cal_hue_rot: f32,
    /// How each Fine tuning slider's strength grows with its position: the
    /// fraction moved is (|slider| / 100) ^ pow (1 = in proportion). Measured
    /// on Chris's food moves (red hue +25, blue sat -15, ...): Lightroom's
    /// small saturation cuts do more, and small hue moves less, than a line
    /// through the +-100 measurements.
    pub cal_hue_pow: f32,
    pub cal_sat_pow: f32,
    /// Saturation of each primary at +100 / -100 (chroma scale - 1).
    pub cal_sat_plus: [f32; 3],
    pub cal_sat_minus: [f32; 3],
    /// Shadow tint: green/magenta shift in stops at Â±100 at black, fading
    /// out between `cal_tint_from` and `cal_tint_to` stops from mid-grey.
    pub cal_tint: f32,
    pub cal_tint_from: f32,
    pub cal_tint_to: f32,
    pub cal_tint_red: f32,
    pub cal_tint_blue: f32,
    /// 1 = shadow tint on the camera's own (white-balanced) channels.
    pub cal_tint_camera: f32,
    /// Shadow tint strength grows as (red x blue white-balance multipliers / 3)
    /// to this power (0 = off).
    pub cal_tint_wb: f32,
    /// Stops the shadow tint darkens per unit, brightness otherwise kept
    /// (below 0: the old behaviour, brightness not kept).
    pub cal_tint_dark: f32,
    /// 1 = turn the camera's own primaries (0 = ProPhoto's).
    pub cal_camera: f32,
    /// Fine tuning Blue Hue also turns the Aqua, Blue and Purple mixer bands
    /// (mixer hue units per Blue Hue unit): Lightroom's Blue Hue -23 turns sky
    /// blue and blue -18..-21 degrees but orange only -4.5, which no primaries
    /// matrix does (ours turned blues -7..-9 for the same orange turn).
    pub cal_blue_mix: [f32; 3],

    /// Mixer: hue shift in degrees at Â±100, per band.
    pub mix_hue: [f32; 8],
    /// Each band's strength for - turns relative to + (Lightroom's differ).
    pub mix_hue_neg: [f32; 8],
    /// Chroma scale - 1 at +100 / -100.
    pub mix_sat_plus: f32,
    pub mix_sat_minus: f32,
    /// OKLab lightness shift at Â±100 for a fully coloured pixel.
    pub mix_lum_plus: f32,
    pub mix_lum_minus: f32,
    /// Chroma (OKLab) at which a colour gets the full effect; greys get none.
    pub mix_chroma_full: f32,
    /// Band width multiplier (1 = each band reaches its neighbours' centres).
    pub mix_width: f32,
    /// How much a luminance change makes colours richer (darker) or paler
    /// (lighter), per unit of relative lightness change.
    pub mix_lum_chroma: f32,
    /// Each band centre's offset from its sRGB angle, degrees.
    pub mix_centre: [f32; 8],
    /// 1 = bands, hue turns and luminance on a ProPhoto RGB colour wheel
    /// (then saturation in OKLab); 0 = all in OKLab.
    pub mix_prophoto: f32,
    /// 1 = the measured Mixer (`mixtable`, Lightroom's response measured on the
    /// synthetic chart) instead of the fitted model, when a table is built in.
    pub mix_table: f32,
    /// 1 = Mixer Hue and Saturation act before the tone controls (Contrast,
    /// Highlights ... Blacks): read and applied on the colour as shown without
    /// them (where the Mixer was measured), then carried back to scene light,
    /// so the sliders act on the mixed colour; Luminance stays last. HSL kits
    /// as before (tools3 / tools4 / mixer5 79 / 76 / 70%); high key indoor
    /// 3.15 -> 3.03 (green bag colour 1.10x -> 0.93x Lightroom's), outdoor
    /// 3.07 -> 2.94 (flower petals lighter). Applying them straight to scene
    /// light won outdoor (2.38) but lost 3-4 points on every HSL kit.
    pub mix_before_tone: f32,
    /// 1 = the measured Grade (`gradetable`) for the Midtones and Global tints
    /// and every Luminance slider; the fitted model keeps Shadows and Highlights.
    pub grade_table: f32,
    /// Each band's luminance strength (multiplies `mix_lum_*`): Lightroom's
    /// differs a lot between colours.
    pub mix_lum_band: [f32; 8],
    /// ProPhoto HSV saturation at which luminance gets its full effect:
    /// Lightroom barely moves dull colours' brightness.
    pub mix_lum_sat_full: f32,
    /// Luminance correction surfaces (brightening, darkening): a factor on
    /// the effect by the pixel's HSV saturation (0.08 .. 1.0) and value (0.25
    /// .. 0.85) on gamma-encoded sRGB, measured on the synthetic chart.
    pub mix_lum_surface_up: [[f32; 8]; 4],
    pub mix_lum_surface_down: [[f32; 8]; 4],
    /// Saturation correction surfaces (slider +, slider -), read like the
    /// Luminance ones: a factor on the effect by the pixel's sRGB HSV
    /// saturation and value, fitted on the synthetic chart.
    pub mix_sat_surface_up: [[f32; 8]; 4],
    pub mix_sat_surface_down: [[f32; 8]; 4],
    /// Each band's Luminance factor by the pixel's sRGB HSV value (0.25, 0.45,
    /// 0.65, 0.85), brightening and darkening, fitted on the synthetic chart.
    pub mix_lum_band_v_up: [[f32; 4]; 8],
    pub mix_lum_band_v_down: [[f32; 4]; 8],
    /// Share of the luminance change a barely coloured pixel still gets.
    pub mix_lum_floor: f32,
    /// The same when darkening (Lightroom darkens faint colours far less).
    pub mix_lum_floor_down: f32,
    /// Shape of the saturation ramp (1 = straight).
    pub mix_lum_sat_pow: f32,
    /// 0 = luminance as a plain gain, 1 = as HSL lightness (pales when
    /// brightened, deepens when darkened).
    pub mix_lum_hsl: f32,
    /// HSL lightness (gamma) by which luminance reaches full effect (0 = off).
    pub mix_lum_dark: f32,
    /// 0 = Saturation scales OKLab chroma, 1 = HSL saturation with HSL
    /// lightness held (saturating darkens blues, as in Lightroom).
    pub mix_sat_hsl: f32,
    pub mix_sat_soft: f32,
    /// Raising saturation: 0 = HSL lightness held, 1 = HSV value held.
    pub mix_sat_hsv: f32,
    /// Exponent on Saturation +'s slider value (1 = linear).
    pub mix_sat_curve: f32,
    /// Encoding gamma of the RGB that Saturation + works in (ProPhoto's 1.8).
    pub mix_sat_gamma: f32,
    /// How far a hue turn follows RGB brightness (0 = keeps OKLab lightness,
    /// 1 = keeps the brightest channel).
    pub mix_hue_value: f32,
    /// Formula Mixer, strong colours: Saturation cuts scaled by 1 +
    /// `mix_sat_strong` x s^2, Luminance's HSL blend raised towards 1 by
    /// `mix_lum_hsl_strong` x s^2 (s: the pixel's ProPhoto HSV saturation), and
    /// the correction surfaces read on ProPhoto (`mix_surface_pp` 1) or sRGB.
    pub mix_sat_strong: f32,
    /// Saturation cuts bite harder on bright strong colours: x (1 + this x s x v^4)
    /// (v: ProPhoto HSV value). The HSL combination kit (12 photos): ours left
    /// bright yellows x1.11-1.13 too colourful after Yellow Saturation -33.
    pub mix_sat_bright: f32,
    /// A Luminance lift also pales bright strong colours: saturation change
    /// - this x lift x s x v^4 (Lightroom's Yellow +22/-33/+28 left the
    /// brightest yellows x1.30 less colourful than ours).
    pub mix_lum_pale: f32,
    pub mix_lum_hsl_strong: f32,
    pub mix_surface_pp: f32,

    /// Curves: the gamma of the space the curves work in (Lightroom works
    /// on gamma-encoded values).
    pub curve_gamma: f32,
    /// How much the RGB curve keeps each colour's hue (0 = bends each
    /// channel on its own, 1 = bends the brightest and darkest channels and
    /// places the middle one in proportion, as the DNG spec describes).
    pub curve_hue_keep: f32,
    /// sRGB-style straight toe on the curves' encoding (0 = pure power).
    pub curve_toe: f32,
    /// 0 = monotone spline through the curve points, 1 = natural cubic.
    pub curve_spline: f32,

    /// Grade: colour push strength at saturation 100, and luminance shift in
    /// stops at Â±100.
    pub grade_sat: f32,
    pub grade_lum: f32,
    /// How Balance moves the shadow/highlight split (exponent per 100).
    pub grade_balance: f32,
    /// Offset (degrees) between Lightroom's hue numbers and our push direction.
    pub grade_hue_offset: f32,
    /// Extra hue offset per range (shadows, mids, highs, global), degrees:
    /// Lightroom's tint for one hue leans differently by range.
    pub grade_range_hue: [f32; 4],
    /// Extra hue offset varying with the hue: cos and sin amplitudes, degrees.
    pub grade_hue_warp: [f32; 2],
    /// Tint strength by hue: cos, sin, cos 2x, sin 2x weights.
    pub grade_hue_gain: [f32; 4],
    /// Which wheel Grade's hues are on: 0 = sRGB, 1 = ProPhoto.
    pub grade_hue_space: f32,
    /// Lightness the tint adds (OKLab L per unit of tint): constant, and
    /// along the tint's a and b directions.
    pub grade_lift: [f32; 3],
    /// Per range, how far colour strength follows a luminance change (1 =
    /// like exposure, 0 = lightness only, below 0 = paler when lifted).
    pub grade_lum_chroma: [f32; 4],
    /// Tint shrinks as 1 / (1 + chroma * this) for already-strong colours.
    pub grade_chroma_damp: f32,
    /// 0 = tints added in OKLab, 1 = added to gamma-encoded sRGB (fades
    /// near black, clips near white); `grade_rgb_gain` its strength.
    pub grade_rgb: f32,
    pub grade_rgb_gain: f32,
    /// The gamma of the RGB the tints are added to.
    pub grade_rgb_gamma: f32,
    /// 1 = add the tints to ProPhoto RGB (0 = sRGB).
    pub grade_rgb_pp: f32,
    /// 0..1: the RGB tint is the hue's pure colour less its grey (1) rather
    /// than the OKLab direction; its gain and hue offset (degrees).
    pub grade_pure: f32,
    pub grade_pure_gain: f32,
    pub grade_pure_offset: f32,
    /// OKLab lightness by which Grade reaches full effect (fades to black below).
    pub grade_black: f32,
    /// OKLab lightness from which Grade fades out towards white (1 = off).
    pub grade_white: f32,
    /// Each range's strength (shadows, mids, highs, global) at lightness
    /// 0.1, 0.3, 0.5, 0.7 and 0.9, measured from Lightroom: its ranges
    /// overlap a lot, a range tint is stronger than the same Global tint,
    /// and the deepest shadows stay nearly untinted.
    pub grade_curves: [[f32; 5]; 4],
    /// The same for each range's Luminance slider, which reaches different
    /// tones than its tint.
    pub grade_lum_curves: [[f32; 5]; 4],
    /// 0..1: share of the Luminance sliders applied per ProPhoto channel
    /// (each judged by its own level, encoded with this gamma), like an RGB
    /// tone curve, rather than to lightness alone.
    pub grade_lum_rgb: f32,
    pub grade_lum_rgb_gamma: f32,
    /// 0..1: in the RGB tint path, how far each channel's range weight
    /// follows that channel's own level (encoded with `grade_rgb_gamma`)
    /// rather than the pixel's lightness, as classic split toning does.
    pub grade_tint_chan: f32,
    /// How far Blending sharpens or softens each range (exponent factor at 0 and 100).
    pub grade_blend: f32,
    /// Per range (shadows, mids, highs, global), a gain on the tints'
    /// lightness change when it brightens and when it darkens: Lightroom's
    /// yellow lifts the mids but not the highlights, its blue darkens the mids
    /// but not the shadows.
    pub grade_lift_up: [f32; 4],
    pub grade_lift_down: [f32; 4],
    /// The tints' lightness gains also scale with (Saturation / 100) to this
    /// power: measured, Lightroom's tint lightness shift is ~0 at 25 and a
    /// quarter of 100's at 50, while the colour push grows in proportion.
    pub grade_lift_satpow: f32,
    /// A factor on the tints by the pixel's HSV saturation (0.08 .. 1.0) and
    /// value (0.25 .. 0.85) on gamma-encoded sRGB, fitted on the synthetic chart.
    pub grade_tint_surface: [[f32; 8]; 4],
    /// Measured corrections per range by the slider settings: at hues 0, 30
    /// .. 330, the tint's gain at Saturation 25 / 50 / 100, a turn of its
    /// direction (degrees) and a factor on the lightness it adds, solved by
    /// least squares over every photo (`DIAG=settable` in toolmatch). Per-
    /// variant fixes showed most of Grade's remaining error follows the
    /// settings, not the photo.
    pub grade_set_gain: [[[f32; 3]; 12]; 4],
    pub grade_set_turn: [[f32; 12]; 4],
    pub grade_set_light: [[f32; 12]; 4],

    /// Vignette: exposure change in stops at Â±100, and where the falloff
    /// starts and ends (radius on an ellipse fitted to the frame: 1 = edges).
    pub vig_amount: f32,
    pub vig_from: f32,
    pub vig_to: f32,

    // Texture, Clarity, Dehaze (definition branch, refitted on Lightroom renders)

    /// Texture: Lightroom works at a fixed size in the photo's own pixels
    /// (about 12 whatever the camera), so the blur radius is in source
    /// pixels (`tex_radius`). The detail (lightness minus its blur, in L*
    /// units) is compressed to a logarithm, so fine low-contrast texture
    /// gains most and strong edges least: change = `tex_amount` x
    /// ln(1 + |detail| / `tex_knee`), `tex_dark` times smaller for detail
    /// darker than its surroundings. The slider acts as |s|^`tex_pow`
    /// (`tex_minus` times smaller below zero), and the shadows and
    /// highlights are spared by `tex_tone_lo` / `tex_tone_hi`.
    /// Clarity favours the mid-tones (`clar_mid`: 0 = all tones alike).
    pub tex_amount: f32,
    pub tex_radius: f32,
    pub tex_knee: f32,
    pub tex_dark: f32,
    pub tex_pow: f32,
    pub tex_minus: f32,
    pub tex_tone_lo: f32,
    pub tex_tone_hi: f32,
    /// Texture's colour detail (a and b, in a* units): gain `tex_chroma`,
    /// eased off for strong detail by `tex_chroma_knee`, and spared in the
    /// highlights by `tex_chroma_hi`.
    pub tex_chroma: f32,
    pub tex_chroma_knee: f32,
    pub tex_chroma_hi: f32,
    /// Clarity above zero is a local-Laplacian filter on lightness (an
    /// edge-aware, region-wise local contrast): differences from each
    /// reference tone are remapped as `clar_sigma` x (|d|/`clar_sigma`)^
    /// `clar_alpha`, and the change at each scale is weighted: fine pyramid
    /// levels (0-1) by `clar_w[0]`, middle (2-4) by `clar_w[1]`, coarse (5+)
    /// by `clar_w[2]`, the base by `clar_w[3]`. Mid-tones are favoured
    /// (`clar_mid`: 0 = all tones alike). The slider acts as |s|^`clar_pow`,
    /// `clar_minus` times smaller below zero; colour strength follows
    /// lightness to the power `clar_cexp`.
    pub clar_mid: f32,
    pub clar_pow: f32,
    pub clar_minus: f32,
    pub clar_cexp: f32,
    /// Below zero Clarity is a different thing: it softens at a medium
    /// radius (`clar_n_radius`, fraction of the long edge) by
    /// `clar_n_amount` of the detail, eased off for strong detail by
    /// `clar_n_knee`.
    /// Plus also pulls each tone towards where an even histogram would put
    /// it (gain `clar_eq` of the difference between its rank and its
    /// lightness): Lightroom's change follows the photo's own histogram.
    pub clar_eq: f32,
    /// 1 = Clarity+ uses the learned lightness model (`clarity_net`), 0 = the local-Laplacian model.
    pub clar_net: f32,
    /// 1 (default) = Clarity+ is the U-Net run on the finished picture (trained on Lightroom renders only), 0 = the older per-pixel network.
    pub clar_disp: f32,
    /// Plus is stronger the wider the photo's base layer (the picture blurred
    /// at `clar_fr` of the long edge) spreads from dark to bright, and the more
    /// detail it has: strength x (range / `clar_rb`)^`clar_pb` x
    /// (detail / `clar_rd`)^`clar_pd`, range = 5th to 95th percentile and
    /// detail = mean difference from the base, both in L* units.
    pub clar_fr: f32,
    pub clar_rb: f32,
    pub clar_pb: f32,
    pub clar_rd: f32,
    pub clar_pd: f32,
    pub clar_sigma: f32,
    pub clar_alpha: f32,
    pub clar_w: [f32; 4],
    pub clar_n_amount: f32,
    pub clar_n_pow: f32,
    pub clar_n_radius: f32,
    pub clar_n_knee: f32,
    /// How strong the softening is at lightness L* 0, 15, 30, 45, 60, 75, 90, 100 (a
    /// fraction of `clar_n_amount`, eased between): Lightroom's Clarity- leaves deep
    /// shadows alone and works hardest in the mid to bright tones.
    pub clar_n_tone: [f32; 8],
    /// Dehaze: how much of the haze colour (estimated from the brightest
    /// parts of the image) is taken out at +100 / mixed in at -100, and a
    /// chroma gain at +100.
    pub dehaze_sat: f32,
    /// Dehaze works on the display image in a lightness-like encoding
    /// (`dz_gamma`). Lightroom's changes follow one curve per photo, with
    /// a strength set by how bright the photo is (`nu`: median of the
    /// encoded luminance; `delta`: mean of its darkest channel). Minus mixes
    /// a haze `dz_haze` (white or brighter) in at strength
    /// `dz_m_amt` x |s|^`dz_m_pow`, scaled by
    /// 1 + med x (nu - 0.45) + dark x (delta - 0.3).
    pub dz_gamma: f32,
    pub dz_haze: f32,
    pub dz_m_amt: f32,
    pub dz_m_pow: f32,
    pub dz_m_med: f32,
    pub dz_m_dark: f32,
    pub dz_p_pow: f32,
    /// Plus follows the dark-channel method: the haze colour A (below), a
    /// transmission t = 1 - `dz_w` x |s|^`dz_p_pow` x (the darkest channel
    /// of the photo against A, eroded over a patch of `dz_patch` of the long
    /// edge and smoothed by `dz_smooth`), at least `dz_t0`, and
    /// J = A + (I - A) / t.
    pub dz_w: f32,
    pub dz_t0: f32,
    pub dz_patch: f32,
    pub dz_smooth: f32,
    /// Haze colour (A): the average of the haziest `dz_a_frac` of the
    /// photo (highest blurred darkest channel). How much its colour (against
    /// grey) colours what plus takes off (`dz_p_col`) and what minus mixes
    /// in (`dz_m_col`).
    pub dz_a_frac: f32,
    /// Plus takes off a haze between grey (0) and A's own colour (1); above 1
    /// takes its tint off harder.
    pub dz_p_col: f32,
    pub dz_m_col: f32,
    /// How much minus follows the dark-channel map (0 = one strength for the
    /// whole photo, 1 = strength proportional to how hazy each place looks).
    pub dz_m_loc: f32,
    /// Minus mixes in a haze that is this much the photo's own haze colour
    /// (the rest: a fixed bright haze).
    pub dz_m_a: f32,
    /// The haze map is smoothed by a blur and by an edge-preserving guided
    /// filter (guided by the photo's own lightness, edge strength `dz_eps`);
    /// `dz_gf` is the share of the guided one.
    pub dz_gf: f32,
    /// The haze map is raised to this power before use (1 = as measured).
    pub dz_dpow: f32,
    pub dz_eps: f32,
}

impl Default for ToolTuning {
    fn default() -> Self {
        Self {
            // Fine tuning fitted 2026-10-07 to Lightroom's Calibration kits
            // (14 photos, 10 cameras, Adobe Color): 72% of Lightroom's change
            // (74% weighted).
            cal_hue: [0.3317, 0.3307, 0.3277],
            cal_hue_rot: 0.9882,
            cal_hue_pow: 1.0759,
            cal_sat_pow: 0.7428,
            cal_sat_plus: [5.0938, 4.8547, 4.3579],
            cal_sat_minus: [0.6663, 0.6667, 0.6643],
            cal_tint: 0.1475,
            cal_tint_from: -4.9303,
            cal_tint_to: 2.3467,
            // Lightroom's magenta shadow tint: less green, red and blue lifted about
            // as much; stronger with the as-shot red x blue white-balance multipliers
            // (`cal_tint_wb`). Refitted 2026-10-08 on 106 photos / 94 camera models.
            cal_tint_red: 0.9729,
            cal_tint_blue: 1.0361,
            cal_tint_camera: 0.0,
            cal_tint_wb: 1.1414,
            cal_tint_dark: 0.8378,
            cal_camera: 0.0,
            cal_blue_mix: [0.15, 0.15, 0.1],
            // Mixer fitted 2026-10-07 to Lightroom's HSL kits (8 colourful
            // photos, 7 cameras, skin tones): 57% of Lightroom's change (64%
            // weighted); luminance as scaled HSL lightness, saturation mostly HSL.
            // Bands and hue turns on a ProPhoto colour wheel (Lightroom's
            // working space): hue turns move colours as Lightroom's do.
            mix_hue: [32.294, 25.502, 32.215, 63.739, 104.84, 44.537, 44.774, 56.731],
            mix_hue_neg: [1.0638, 1.1078, 0.79344, 0.52003, 0.53203, 1.3717, 0.97617, 0.65977],
            mix_sat_plus: 1.2244,
            mix_sat_minus: 0.94938,
            mix_lum_plus: 1.0553,
            mix_lum_minus: 1.7431,
            mix_chroma_full: 0.0072,
            mix_width: 0.89732,
            mix_lum_chroma: 0.05,
            mix_hue_value: 1.15,
            mix_sat_strong: 1.1125,
            mix_sat_bright: 0.20313,
            mix_lum_pale: 0.6875,
            mix_lum_hsl_strong: 1.0938,
            mix_surface_pp: 1.0,
            // Band centres' offsets on the ProPhoto wheel.
            mix_centre: [-0.07375, 2.2319, -3.1447, -30.0, -29.434, -6.6403, 5.4732, 21.216],
            mix_prophoto: 1.0,
            mix_table: 0.0,
            mix_before_tone: 1.0,
            grade_table: 0.0,
            mix_lum_band: [1.2086, 1.3461, 1.2991, 1.3682, 1.6406, 1.2231, 1.0145, 1.6192],
            mix_lum_sat_full: 0.51479,
            // Fitted 2026-10-08 on the synthetic chart's Luminance variants only; the 27 kit
            // photos, held out, went 69.2% -> 70.8%. Rows V 0.25 .. 0.85, columns S 0.08 .. 1.0.
            mix_lum_surface_up: [[0.6375, 0.975, 1.1188, 1.3125, 1.4068, 2.531, 1.225, 2.288], [0.65, 1.075, 1.075, 1.075, 0.775, 0.9375, 0.25, 1.2], [1.2375, 1.4313, 1.4877, 1.3938, 1.0875, 1.0255, 2.2875, 0.95], [0.6875, 0.81875, 0.675, 0.8125, 0.725, 0.6315, 0.8, 0.975]],
            mix_lum_surface_down: [[0.65, 0.8375, 0.88125, 0.9875, 1.1125, 2.113, 0.6875, 1.725], [0.775, 0.90625, 0.9375, 0.9125, 0.775, 1.1938, 0.7125, 1.1], [1.5875, 1.65, 1.5063, 1.1375, 1.188, 1.325, 0.65, 1.025], [0.8, 1.144, 1.1375, 1.3, 0.7, 0.725, 0.225, 1.05]],
            mix_sat_surface_up: [[1.0516, 0.80313, 0.86438, 0.73262, 0.40506, 0.20287, 0.208, 0.456], [1.1969, 1.1687, 1.2156, 0.90625, 0.5875, 0.20313, 0.21275, 0.738], [1.0, 0.97213, 0.93931, 0.80288, 0.43281, 0.38125, 0.20325, 0.738], [0.73244, 0.86875, 1.0887, 0.81707, 0.475, 0.20313, 0.25025, 0.738]],
            mix_lum_band_v_up: [
                [1.15, 1.15, 0.925, 1.075],
                [1.15, 1.0, 0.78, 0.925],
                [1.0, 1.0, 1.0, 0.906],
                [0.85, 0.906, 0.939, 0.836],
                [1.0, 1.262, 1.375, 1.291],
                [1.0, 1.0, 1.0, 0.963],
                [1.206, 1.15, 1.094, 1.094],
                [1.375, 1.375, 1.15, 1.375],
            ],
            mix_lum_band_v_down: [
                [1.15, 1.15, 1.094, 1.15],
                [1.0, 1.0, 0.906, 0.906],
                [1.0, 1.0, 0.963, 0.925],
                [0.906, 0.906, 0.92, 0.963],
                [1.0, 1.262, 1.375, 1.544],
                [1.0, 1.0, 1.075, 0.953],
                [1.206, 1.094, 1.047, 1.0],
                [1.375, 1.262, 1.206, 1.333],
            ],
            mix_sat_surface_down: [[0.96719, 0.85938, 0.92081, 0.86456, 0.76612, 0.62525, 0.30625, 1.0], [0.93906, 0.97656, 0.98619, 0.86456, 0.79425, 0.61144, 0.4005, 1.038], [0.93438, 1.0424, 0.92995, 0.89737, 0.73712, 0.6065, 0.52707, 1.038], [1.0563, 1.019, 1.0048, 0.8875, 0.83606, 0.73763, 1.0283, 1.122]],
            mix_lum_floor: 0.11512,
            mix_lum_floor_down: 0.011328,
            mix_lum_sat_pow: 0.90726,
            mix_lum_hsl: 0.57214,
            mix_lum_dark: 0.39825,
            mix_sat_hsl: 0.99775,
            mix_sat_soft: 0.92624,
            mix_sat_hsv: 0.9848,
            mix_sat_curve: 1.4336,
            mix_sat_gamma: 1.1782,
            // Curves fitted 2026-10-07 (14 photos, 10 cameras), on ProPhoto's
            // channels: 88% of Lightroom's change (91% weighted).
            curve_gamma: 2.2451,
            curve_hue_keep: 0.75,
            curve_toe: 0.0365625,
            curve_spline: 0.8125,
            // Grade fitted 2026-10-07 on Lightroom's own base render, two
            // rounds of kits (orange/blue on 14 photos, then six hues at three
            // strengths): 75% of its change (weighted). Tints partly the hue's
            // pure colour added to encoded sRGB, partly OKLab.
            grade_sat: 0.07,
            grade_lum: 0.277,
            grade_balance: 2.1446,
            grade_hue_offset: -18.832,
            grade_range_hue: [12.606, 14.277, 11.56, 15.152],
            grade_hue_warp: [-1.6247, -2.3125],
            grade_hue_gain: [0.0113, -0.0162, 0.0932, -0.2856],
            grade_hue_space: 0.8739,
            grade_lift: [0.0163, -0.1561, 0.5525],
            grade_lum_chroma: [-0.5869, 2.1483, -2.2556, 1.0875],
            grade_chroma_damp: 1.9701,
            grade_rgb: 0.1907,
            grade_rgb_gain: 1.8167,
            grade_rgb_gamma: 3.1646,
            grade_rgb_pp: 1.0,
            grade_pure: 0.3004,
            grade_pure_gain: 5.1923,
            grade_pure_offset: 0.5863,
            grade_black: 0.3675,
            grade_white: 0.8175,
            // Fitted from Lightroom's measured tint size per range and
            // lightness (shadows, mids, highs, global at L 0.1 .. 0.9).
            grade_curves: [[2.1333, 1.8239, 1.7642, 1.0241, 0.0483], [0.0, 0.1149, 0.9171, 1.122, 0.0907], [0.0, 0.1191, 0.883, 1.624, 2.1522], [0.3548, 0.9836, 1.2239, 1.2947, 1.031]],
            // Luminance refitted 2026-10-08 with the per-channel path (BASE=lr,
            // Luminance variants 63% -> 81%): Lightroom's Grade Luminance acts
            // like an RGB tone curve on each ProPhoto channel.
            grade_lum_curves: [[9.55, 3.5927, 0.7206, 0.235, 0.0042], [0.0, 0.7932, 2.2264, 1.1495, 0.003], [0.0002, 0.0998, 0.332, 1.077, 1.9444], [2.5252, 2.4225, 2.1362, 1.5848, 0.6513]],
            grade_lum_rgb: 0.9502,
            grade_lum_rgb_gamma: 2.6301,
            // Fitted 2026-10-08 (BASE=lr, Grade 80% -> 82%): Lightroom weights
            // its RGB tint per channel by the channel's own level.
            grade_tint_chan: 0.9766,
            // < 1: higher Blending gives crisper ranges, as measured.
            grade_blend: 0.4777,
            // Fitted 2026-10-08 on BASE=lr (total error 2013 -> 1989): tints
            // barely darken the shadows or brighten the highlights.
            grade_lift_up: [0.6602, 1.4805, 0.3578, 1.2157],
            grade_lift_down: [0.0016, 1.6563, 1.3727, 1.6773],
            // Refitted 2026-10-08 with round-4 kits (total error 4305 -> 4248).
            grade_lift_satpow: 0.6445,
            grade_tint_surface: [
                [0.938, 0.919, 0.975, 0.95, 0.919, 0.919, 0.919, 0.975],
                [1.012, 1.006, 1.006, 1.025, 1.012, 1.05, 1.025, 1.081],
                [0.975, 1.025, 1.062, 1.081, 1.106, 1.162, 1.162, 1.162],
                [0.969, 0.975, 0.994, 1.062, 1.125, 1.125, 1.125, 1.181],
            ],
            grade_set_gain: [[[0.996, 0.983, 1.061], [0.944, 0.968, 0.899], [1.147, 1.190, 1.213], [1.049, 1.078, 1.100], [1.056, 1.062, 1.134], [0.934, 0.931, 0.995], [1.078, 1.050, 1.222], [0.937, 0.966, 0.958], [0.937, 0.901, 0.995], [0.910, 0.893, 0.977], [0.962, 0.985, 1.139], [0.840, 0.854, 0.955]],
    [[1.096, 1.090, 1.092], [0.960, 0.950, 0.945], [1.100, 1.109, 1.143], [1.027, 1.037, 1.069], [1.089, 1.105, 1.159], [0.926, 0.948, 0.999], [1.083, 1.107, 1.178], [0.909, 0.929, 0.970], [1.061, 1.066, 1.086], [0.976, 0.979, 0.994], [1.043, 1.050, 1.086], [0.909, 0.908, 0.926]],
    [[1.047, 1.082, 1.150], [0.887, 0.905, 0.914], [0.878, 0.868, 0.979], [0.887, 0.887, 0.982], [0.999, 1.014, 1.139], [0.868, 0.884, 0.992], [0.969, 0.993, 1.080], [0.826, 0.860, 0.894], [0.908, 1.013, 1.112], [0.871, 0.913, 0.956], [1.003, 1.037, 1.092], [0.888, 0.905, 0.945]],
    [[1.010, 1.007, 1.048], [0.904, 0.896, 0.896], [1.019, 1.031, 1.077], [0.977, 0.989, 1.031], [1.017, 1.036, 1.109], [0.878, 0.905, 0.970], [1.000, 1.031, 1.113], [0.865, 0.893, 0.948], [0.988, 0.999, 1.036], [0.930, 0.935, 0.963], [0.971, 0.980, 1.045], [0.857, 0.859, 0.901]]],
grade_set_turn: [[-2.129, 7.927, 10.001, 7.481, 7.397, 7.561, 0.927, -1.575, 12.313, 12.642, 5.872, -0.079],
    [-4.038, -2.475, 5.057, 9.661, 10.352, 5.904, -1.005, -0.911, 9.207, 9.120, 7.224, 1.776],
    [-1.963, -2.904, 7.340, 15.949, 12.193, 7.864, 0.677, 3.976, 11.680, 10.594, 9.846, 4.951],
    [-4.122, -1.753, 3.298, 7.780, 9.003, 5.641, -0.754, -0.636, 7.044, 6.756, 5.052, 0.698]],
grade_set_light: [[0.212, 1.622, 2.851, 1.571, 0.748, 1.127, 1.699, 2.769, 6.366, 2.599, 0.039, 0.058],
    [0.605, 1.004, 1.954, 0.987, 0.387, 0.573, 0.847, 0.852, 1.583, 0.879, 0.308, 0.472],
    [0.907, 0.796, 2.345, 0.898, 0.050, 0.336, 0.734, 0.899, 1.641, 1.135, 0.402, 0.622],
    [0.750, 1.306, 2.238, 1.167, 0.445, 0.662, 1.040, 0.915, 1.606, 0.938, 0.323, 0.562]],
            // Vignette fitted 2026-10-07 (D810 garden), elliptical like
            // Lightroom's at Roundness 0: 65%.
            vig_amount: 7.075,
            vig_from: 0.7,
            vig_to: 1.25,
            // Definition fitted 2026-10-07 (D810 garden), compared at 300px, so
            // Texture's fine detail is barely measured: 24% overall, a first pass.
            tex_amount: 1.488,
            tex_radius: 10.17,
            tex_knee: 1.303,
            tex_dark: 0.82,
            tex_pow: 0.78,   // 0.685 made +-25 17-29% too strong against Lightroom (3 photos x 6 amounts); +-100 is unaffected
            tex_minus: 0.8738,
            tex_tone_lo: 0.9937,
            tex_tone_hi: 0.6187,
            tex_chroma: 0.7995,
            tex_chroma_knee: 39.37,
            tex_chroma_hi: 0.2531,
            clar_mid: 1.2458,
            clar_pow: 0.9513,
            clar_minus: 0.8573,
            clar_cexp: 0.6969,
            clar_fr: 0.0371,
            clar_rb: 48.97,
            clar_pb: 0.4828,
            clar_rd: 5.555,
            clar_pd: 0.5538,
            clar_eq: 0.1181,
            clar_net: 1.0,
            clar_disp: 1.0,
            clar_sigma: 0.602,
            clar_alpha: 0.7335,
            clar_w: [1.008, 0.7541, 0.8095, -0.1102],
            clar_n_amount: 0.5619,
            clar_n_pow: 0.7684,
            clar_n_radius: 0.01316,
            clar_n_tone: [0.14, -0.015, 0.3271, 0.9484, 1.19, 1.082, 0.8287, 0.3115],
            clar_n_knee: 540.1,
            dehaze_sat: 0.01621,
            dz_gamma: 1.903,
            dz_haze: 0.9101,
            dz_m_amt: 0.6548,
            dz_m_pow: 1.248,
            dz_m_med: 0.4118,
            dz_m_dark: 2.479,
            dz_p_pow: 0.9245,
            dz_w: 0.9289,
            dz_t0: 0.096,
            dz_patch: 0.006,
            dz_smooth: 0.08335,
            dz_a_frac: 0.00615,
            dz_gf: 0.9988,
            dz_dpow: 1.135,
            dz_eps: 0.04827,
            dz_m_a: 0.5892,
            dz_m_loc: 0.7315,
            dz_p_col: 1.441,
            dz_m_col: 1.749,
        }
    }
}

fn apply3(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

// The legacy (Lightroom post-crop, Highlight priority) vignette: RapidOne's
// formula, its constants fitted on a Lightroom kit (16 Oct: 12 photos x 18
// slider settings, `tools/ml/vignette_global.py`; held-out error 0.23 stops
// against 0.60 for the old exposure ramp, on changes of 1.4 stops).
// Darkening: linear brightness x (1 - s f)^K, eased on bright parts;
// lightening: 1 - (1 - y)(1 - s f)^K_PLUS (dark parts lift most).
// f = smoothstep(r; centre -/+ half)^gamma, all three from Midpoint / Feather.
pub(crate) const VIG_K: f32 = 3.524;
pub(crate) const VIG_K_PLUS: f32 = 0.547;
pub(crate) const VIG_CENTRE: [f32; 3] = [0.9996, 0.3364, 0.0151];
pub(crate) const VIG_HALF: [f32; 3] = [0.2530, 0.5566, -0.3808];
pub(crate) const VIG_GAMMA: [f32; 3] = [-0.2104, 0.3178, 0.0908];
/// Roundness +: towards a circle (as before), this much of the way.
pub(crate) const VIG_ROUND_POS: f32 = 0.2962;
/// Roundness -: a p-norm (p = 2 + a x + b x^2, x = -Roundness/100) scaled
/// out by 1 + c x + d x^2 (the -50 / -100 fits, joined to the oval at 0).
pub(crate) const VIG_ROUND_P: [f32; 2] = [-1.13, 4.82];
pub(crate) const VIG_ROUND_SCALE: [f32; 2] = [-0.13, 0.356];
/// Easing on bright parts by brightness (linear luminance at these knots),
/// at Highlights 0 plus Highlights/100 x the second row.
pub(crate) const VIG_KNOTS: [f32; 9] = [0.0, 0.05, 0.1, 0.25, 0.4, 0.55, 0.7, 0.85, 1.0];
pub(crate) const VIG_EASE0: [f32; 9] = [0.2417, -0.2373, -0.1295, 0.1015, 0.2975, 0.4493, 0.5731, 0.7223, 0.8635];
pub(crate) const VIG_EASE1: [f32; 9] = [-0.1702, -1.6756, -1.2573, -0.1770, 0.0423, 0.1907, 0.2705, 0.2789, 0.3211];

/// The vignette's (centre, half width, gamma) from Midpoint and Feather.
fn vig_shape(s: &ToolSettings) -> (f32, f32, f32) {
    let q = |c: [f32; 3], x: f32| c[0] + c[1] * x + c[2] * x * x;
    let (m, fe) = ((s.vignette_midpoint - 50.0) / 50.0, (s.vignette_feather - 50.0) / 50.0);
    let softplus = |x: f32| if x > 20.0 { x } else { x.exp().ln_1p() };
    let half = softplus(q(VIG_HALF, fe) * 4.0) / 4.0 + 0.005;
    (q(VIG_CENTRE, m), half, q(VIG_GAMMA, fe).exp().clamp(0.3, 8.0))
}

/// How much a darkening vignette spares a pixel of linear luminance `y` (0..1).
fn vig_ease(y: f32, highlights: f32) -> f32 {
    let y = y.clamp(0.0, 1.0 - 1e-6);
    let i = VIG_KNOTS.iter().rposition(|&k| k <= y).unwrap_or(0).min(VIG_KNOTS.len() - 2);
    let f = (y - VIG_KNOTS[i]) / (VIG_KNOTS[i + 1] - VIG_KNOTS[i]);
    let g = |j: usize| VIG_EASE0[j] + highlights * VIG_EASE1[j];
    let g = g(i) * (1.0 - f) + g(i + 1) * f;
    (2.0 / (1.0 + (3.0 - 3.0 * g).exp())).min(1.0)
}

fn ramp(x: f32, from: f32, to: f32) -> f32 {
    let t = ((x - from) / (to - from).max(1e-3)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---- Fine tuning (Calibration) ------------------------------------------------

fn mul3(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// Linear Rec.2020 (D65) -> linear ProPhoto (D50), Lightroom's working space.
pub fn rec2020_to_prophoto() -> Mat3 {
    use crate::dngcolour::{PROPHOTO_TO_XYZ_D50, XYZ_D65_TO_D50};
    mul3(&crate::whitebalance::invert3(&PROPHOTO_TO_XYZ_D50), &mul3(&XYZ_D65_TO_D50, &crate::pipeline::REC2020_TO_XYZ))
}

/// The primaries matrix for scene-linear Rec.2020, built in ProPhoto (the
/// wide space Lightroom works in - in a narrower one, colours like cyan
/// water sit with green equal to blue and barely move): each primary moves
/// towards its neighbour for hue (+: redâ†’yellow, greenâ†’cyan, blueâ†’magenta)
/// and away from or towards grey for saturation; rows are normalised so
/// white stays white.
///
/// `camera` is (Rec.2020 -> white-balanced camera RGB, and back): with
/// `cal_camera` set the primaries turned are the camera's own, as Lightroom's
/// Calibration adjusts the camera profile, so the effect differs by camera.
pub fn calibration_matrix(s: &ToolSettings, t: &ToolTuning, camera: Option<&(Mat3, Mat3)>) -> Option<Mat3> {
    let m = primaries_matrix(s, t)?;
    if let (Some((to_cam, from_cam)), true) = (camera, t.cal_camera > 0.5) {
        return Some(mul3(from_cam, &mul3(&m, to_cam)));
    }
    let to_pp = rec2020_to_prophoto();
    Some(mul3(&crate::whitebalance::invert3(&to_pp), &mul3(&m, &to_pp)))
}

fn primaries_matrix(s: &ToolSettings, t: &ToolTuning) -> Option<Mat3> {
    let hue = [s.cal_red_hue, s.cal_green_hue, s.cal_blue_hue];
    let sat = [s.cal_red_sat, s.cal_green_sat, s.cal_blue_sat];
    if hue.iter().chain(&sat).all(|&v| v == 0.0) {
        return None;
    }
    let mut cols = [[0.0f32; 3]; 3];
    for i in 0..3 {
        let mut p = [0.0f32; 3];
        p[i] = 1.0;
        // A hue turn towards the next primary (or the previous, for -): mix
        // that one in and take some of the third out, so the turn moves
        // every colour - cyan water included, which mixing alone can't touch.
        let h = hue[i].signum() * (hue[i].abs() / 100.0).powf(t.cal_hue_pow) * t.cal_hue[i];
        let (towards, away) = if h > 0.0 { ((i + 1) % 3, (i + 2) % 3) } else { ((i + 2) % 3, (i + 1) % 3) };
        p[towards] += h.abs();
        p[away] -= h.abs() * t.cal_hue_rot;
        let grey = (p[0] + p[1] + p[2]) / 3.0;
        // The curve applies to cuts only: no kit measures moderate boosts yet,
        // and a curved boost overshot (Chris's high key: Green / Blue sat +8).
        let k = if sat[i] >= 0.0 { 1.0 + sat[i] / 100.0 * t.cal_sat_plus[i] } else { 1.0 - (sat[i].abs() / 100.0).powf(t.cal_sat_pow) * t.cal_sat_minus[i] };
        cols[i] = p.map(|v| grey + (v - grey) * k.max(0.0));
    }
    let m: Mat3 = std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r]));
    // Keep white white by scaling each new primary (column), as when an RGB
    // space is built from new primaries and a white point. (Rescaling rows
    // instead quietly cancels most of a saturation change.)
    let inv = crate::whitebalance::invert3(&m);
    let s: [f32; 3] = std::array::from_fn(|i| inv[i][0] + inv[i][1] + inv[i][2]);
    if s.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Some(m);
    }
    Some(std::array::from_fn(|r| std::array::from_fn(|c| m[r][c] * s[c])))
}

/// Shadow tint in scene light: green (-) or magenta (+), strongest at black.
/// `camera` is (Rec.2020 -> white-balanced camera RGB, and back): with it the
/// tint works on the camera's own channels, so its strength and colour
/// differ by camera as Lightroom's do.
pub fn shadow_tint(c: [f32; 3], amount: f32, t: &ToolTuning, camera: Option<&(Mat3, Mat3)>, wb_gain: f32) -> [f32; 3] {
    if amount == 0.0 {
        return c;
    }
    let l = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    let ev = (l.max(1e-6) / 0.18).log2();
    let w = 1.0 - ramp(ev, t.cal_tint_from, t.cal_tint_to);
    // Stronger the more the white balance lifts red and blue (`wb_gain` = their
    // as-shot multipliers multiplied, ~3 in daylight): Lightroom seems to add
    // the tint to the raw channels before white balance.
    let a = amount / 100.0 * t.cal_tint * w * (wb_gain / 3.0).max(0.1).powf(t.cal_tint_wb);
    // Magenta = less green, plus some red and blue lift (`cal_tint_red`,
    // `cal_tint_blue`; Lightroom's leans blue and also darkens a little).
    let tint = |c: [f32; 3]| [c[0] * 2f32.powf(a * t.cal_tint_red), c[1] * 2f32.powf(-a), c[2] * 2f32.powf(a * t.cal_tint_blue)];
    let out = match camera {
        Some((to_cam, from_cam)) if t.cal_tint_camera > 0.5 => apply3(from_cam, tint(apply3(to_cam, c))).map(|x| x.max(0.0)),
        _ => tint(c),
    };
    // Lightroom's tint darkens a little either way, so keep the brightness
    // and take `cal_tint_dark` stops off per unit of tint.
    let l2 = LUMA[0] * out[0] + LUMA[1] * out[1] + LUMA[2] * out[2];
    if l2 <= 1e-9 || t.cal_tint_dark < 0.0 {
        return out;
    }
    let k = l / l2 * 2f32.powf(-a.abs() * t.cal_tint_dark);
    out.map(|x| x * k)
}

// ---- Display-side tools ---------------------------------------------------------

/// RGB -> hue in degrees (HSL/HSV hue).
fn rgb_hue([r, g, b]: [f32; 3]) -> Option<f32> {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d <= 1e-6 {
        return None;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    Some(h * 60.0)
}

/// Weight of each mixer band at a hue: linear between neighbouring band
/// centres (they sum to 1), widened or narrowed by `width`. `shift` moves
/// each centre (degrees): Lightroom's bands sit where its own colour space
/// puts them, not exactly at the sRGB angles.
fn band_weights(hue: f32, width: f32, shift: &[f32; 8]) -> [f32; 8] {
    let centre = |b: usize| BAND_CENTRES[b] + shift[b];
    let mut w = [0.0f32; 8];
    for b in 0..8 {
        let c = centre(b);
        let prev = centre((b + 7) % 8);
        let next = centre((b + 1) % 8);
        let d = (hue - c + 540.0).rem_euclid(360.0) - 180.0; // signed distance
        let span = if d >= 0.0 { (next - c + 360.0).rem_euclid(360.0) } else { (c - prev + 360.0).rem_euclid(360.0) };
        w[b] = (1.0 - d.abs() / (span * width).max(1e-3)).max(0.0);
    }
    let sum: f32 = w.iter().sum();
    if sum > 0.0 {
        w.iter_mut().for_each(|v| *v /= sum);
    }
    w
}

/// Everything prepared once per render for the display-side tools.
pub struct Prepared {
    pub(crate) mixer: Option<[[f32; 3]; 8]>,
    /// Luminosity, RGB, red, green, blue.
    pub(crate) curves: Option<[Option<Curve>; 5]>,
    /// The parametric (region) curve, on gamma-2.2 ProPhoto.
    pub(crate) parametric: Option<Curve>,
    pub(crate) grade: Option<GradeSettings>,
    /// The grade's settings worked out once (`GradePrep`): the whole grade, and the fitted part the
    /// measured table is used with (its Shadows and Highlights tints; None when they're neutral).
    pub(crate) grade_full: Option<GradePrep>,
    pub(crate) grade_fitted: Option<GradePrep>,
    pub(crate) vignette: f32,
    /// The adaptive vignette's amount (-100..100).
    pub(crate) adaptive: f32,
    /// The legacy vignette's fade (`vig_shape`: centre and half width on
    /// the radius, 1 = the frame's edge oval; gamma), and its roundness
    /// (-1 squarer .. 1 circular).
    pub(crate) vig_centre: f32,
    pub(crate) vig_half: f32,
    pub(crate) vig_gamma: f32,
    pub(crate) vig_round: f32,
    /// 0..1: how much bright parts are spared a darkening vignette.
    pub(crate) vig_highlights: f32,
    /// Width / height, for a circular vignette.
    pub aspect: f32,
    /// Subject-aware vignette: how much to darken each part of the frame.
    pub(crate) vig_map: Option<std::sync::Arc<crate::subject::VignetteMap>>,
    pub(crate) to_srgb: Mat3,
    pub(crate) from_srgb: Mat3,
    pub(crate) to_prophoto: Mat3,
    pub(crate) from_prophoto: Mat3,
    pub(crate) t: ToolTuning,
}

fn curve_of(points: &[[f32; 2]], t: &ToolTuning) -> Option<Curve> {
    let straight = points.len() < 2 || points.iter().all(|p| (p[0] - p[1]).abs() < 0.5);
    (!straight).then(|| Curve::natural(&points.iter().map(|p| (p[0] / 255.0, p[1] / 255.0)).collect::<Vec<_>>(), t.curve_spline))
}

impl Prepared {
    pub fn new(s: &ToolSettings, t: &ToolTuning) -> Prepared {
        let curves = [
            curve_of(&s.curve_luma, t),
            curve_of(&s.curve_rgb, t),
            curve_of(&s.curve_red, t),
            curve_of(&s.curve_green, t),
            curve_of(&s.curve_blue, t),
        ];
        let ok = Oklab::new();
        let g = &s.grade;
        let ranges = [g.shadows, g.mids, g.highs, g.global];
        let grade_dirs: [[f32; 2]; 4] = std::array::from_fn(|i| {
            // The offset also varies with the hue itself (`grade_hue_warp`).
            let h = ranges[i].hue.to_radians();
            let warp = t.grade_hue_warp[0] * h.cos() + t.grade_hue_warp[1] * h.sin();
            hue_direction(ranges[i].hue + t.grade_hue_offset + t.grade_range_hue[i] + warp, t.grade_hue_space, &ok)
        });
        let (to_srgb, to_prophoto) = (rec2020_to_srgb(), rec2020_to_prophoto());
        let (from_srgb, from_prophoto) = (crate::whitebalance::invert3(&to_srgb), crate::whitebalance::invert3(&to_prophoto));
        let spaces = [&to_srgb, &from_srgb, &to_prophoto, &from_prophoto];
        let graded = !s.grade.is_neutral();
        let grade_full = graded.then(|| GradePrep::new(&s.grade, t, &ok, &grade_dirs, spaces));
        let grade_fitted = graded.then(|| fitted_grade(&s.grade)).filter(|f| !f.is_neutral()).map(|f| GradePrep::new(&f, t, &ok, &grade_dirs, spaces));
        Prepared {
            grade_full,
            grade_fitted,
            mixer: {
                let mut m = s.mixer;
                for (k, b) in [4, 5, 6].into_iter().enumerate() {
                    m[b][0] = (m[b][0] + s.cal_blue_hue * t.cal_blue_mix[k]).clamp(-100.0, 100.0);
                }
                m.iter().flatten().any(|&v| v != 0.0).then_some(m)
            },
            curves: curves.iter().any(Option::is_some).then_some(curves),
            parametric: parametric_curve(s),
            grade: (!s.grade.is_neutral()).then_some(s.grade),
            vignette: s.vignette,
            adaptive: s.adaptive_vignette,
            vig_centre: vig_shape(s).0,
            vig_half: vig_shape(s).1,
            vig_gamma: vig_shape(s).2,
            vig_round: (s.vignette_roundness / 100.0).clamp(-1.0, 1.0),
            vig_highlights: (s.vignette_highlights / 100.0).clamp(0.0, 1.0),
            aspect: 1.0,
            vig_map: s.vignette_map.clone(),
            to_srgb: rec2020_to_srgb(),
            from_srgb: crate::whitebalance::invert3(&rec2020_to_srgb()),
            to_prophoto: rec2020_to_prophoto(),
            from_prophoto: crate::whitebalance::invert3(&rec2020_to_prophoto()),
            t: *t,
        }
    }

    pub fn is_identity(&self) -> bool {
        self.mixer.is_none() && self.curves.is_none() && self.parametric.is_none() && self.grade.is_none() && self.vignette == 0.0 && self.adaptive == 0.0
    }

    /// Applies Mixer, Curves, Grade and Vignette to a display-linear
    /// Rec.2020 colour at position (u, v) - each -1..1 across the frame.
    pub fn apply(&self, c: [f32; 3], ok: &Oklab, u: f32, v: f32) -> [f32; 3] {
        self.apply_after_mixer(self.apply_mixer(c, ok), ok, u, v)
    }

    /// The Mixer alone (on a display-linear Rec.2020 colour).
    pub fn apply_mixer(&self, c: [f32; 3], ok: &Oklab) -> [f32; 3] {
        let Some(mix) = &self.mixer else { return c };
        self.mixer_with(c, ok, mix)
    }

    /// The Mixer with only some of its controls (`keep`: hue, saturation, luminance).
    /// Whether `apply_mixer_part` with `keep` changes nothing (no Mixer, or none of those parts moved).
    pub fn mixer_part_is_identity(&self, keep: [bool; 3]) -> bool {
        self.mixer.as_ref().is_none_or(|mix| mix.iter().all(|band| (0..3).all(|k| !keep[k] || band[k] == 0.0)))
    }

    pub fn apply_mixer_part(&self, c: [f32; 3], ok: &Oklab, keep: [bool; 3]) -> [f32; 3] {
        let Some(mix) = &self.mixer else { return c };
        let part: [[f32; 3]; 8] = std::array::from_fn(|b| std::array::from_fn(|k| if keep[k] { mix[b][k] } else { 0.0 }));
        if part.iter().flatten().all(|&v| v == 0.0) {
            return c;
        }
        self.mixer_with(c, ok, &part)
    }

    fn mixer_with(&self, c: [f32; 3], ok: &Oklab, mix: &[[f32; 3]; 8]) -> [f32; 3] {
        let t = &self.t;
        let measured = if t.mix_table > 0.5 { crate::mixtable::table() } else { None };
        if let Some(table) = measured {
            apply3(&self.from_prophoto, table.apply(apply3(&self.to_prophoto, c), mix))
        } else if t.mix_prophoto > 0.5 {
            mixer_prophoto(c, mix, t, ok, &self.to_prophoto, &self.from_prophoto, &self.to_srgb)
        } else {
            mixer(c, mix, t, ok, &self.to_srgb)
        }
    }

    /// Curves, Grade and Vignette (everything after the Mixer).
    pub fn apply_after_mixer(&self, mut c: [f32; 3], ok: &Oklab, u: f32, v: f32) -> [f32; 3] {
        let t = &self.t;
        // The parametric curve comes before the point curves, as in Lightroom:
        // one curve on gamma-2.2 ProPhoto, applied keeping hue (brightest and
        // darkest channel curved, the middle one keeping its place).
        if let Some(pc) = &self.parametric {
            let e = apply3(&self.to_prophoto, c).map(|x| x.clamp(0.0, 1.0).powf(1.0 / 2.2));
            c = apply3(&self.from_prophoto, pc.apply_rgb(e).map(|x| x.max(0.0).powf(2.2))).map(|x| x.max(0.0));
        }
        if let Some(curves) = &self.curves {
            let enc = Encoding::new(t.curve_gamma, t.curve_toe);
            if let Some(luma) = &curves[0] {
                // Brightness only: the curve on luminance, colour scaled with it.
                let l = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
                if l > 1e-6 {
                    let new = enc.decode(luma.eval(enc.encode(l.min(1.0))).max(0.0));
                    c = c.map(|x| x * new / l);
                }
            }
            // The RGB and channel curves work on ProPhoto's red, green and
            // blue, as in Lightroom (its working space): measured 89% of
            // Lightroom's change against 82% on Rec.2020's channels.
            let mut e = apply3(&self.to_prophoto, c).map(|x| enc.encode(x.clamp(0.0, 1.0)));
            if let Some(rgb) = &curves[1] {
                let each = e.map(|x| rgb.eval(x));
                e = if t.curve_hue_keep > 0.0 {
                    let kept = rgb.apply_rgb(e);
                    std::array::from_fn(|i| each[i] + (kept[i] - each[i]) * t.curve_hue_keep)
                } else {
                    each
                };
            }
            for ch in 0..3 {
                if let Some(cv) = &curves[ch + 2] {
                    e[ch] = cv.eval(e[ch]);
                }
            }
            c = apply3(&self.from_prophoto, e.map(|x| enc.decode(x.max(0.0)))).map(|x| x.max(0.0));
        }
        if let Some(gr) = &self.grade {
            let spaces = [&self.to_srgb, &self.from_srgb, &self.to_prophoto, &self.from_prophoto];
            let measured = if t.grade_table > 0.5 { crate::gradetable::table() } else { None };
            c = if let Some(table) = measured {
                // Hybrid: the fitted model for the Shadows and Highlights tints
                // (with Blending and Balance), where the chart can't stand in
                // for a RAW; the measured per-channel curves for the Midtones
                // and Global tints and every Luminance slider, which carry over.
                let c = match &self.grade_fitted { Some(prep) => grade_prepared(c, prep, t, ok, spaces), None => c };
                let ranges = [
                    (gr.shadows.hue, 0.0, gr.shadows.lum),
                    (gr.mids.hue, gr.mids.sat, gr.mids.lum),
                    (gr.highs.hue, 0.0, gr.highs.lum),
                    (gr.global.hue, gr.global.sat, gr.global.lum),
                ];
                apply3(&self.from_prophoto, table.apply(apply3(&self.to_prophoto, c), &ranges))
            } else {
                grade_prepared(c, self.grade_full.as_ref().expect("made with the grade"), t, ok, spaces)
            };
        }
        let mut a = 0.0;
        if self.vignette != 0.0 {
            // Legacy (Lightroom) vignette, as fitted (see VIG_K). Roundness +: towards a
            // circle (the long side scaled to match the short); -: towards a rectangle.
            let r = if self.vig_round > 0.0 {
                let k = self.vig_round * VIG_ROUND_POS;
                let (u2, v2) = if self.aspect >= 1.0 { (u * (1.0 + (self.aspect - 1.0) * k), v) } else { (u, v * (1.0 + (1.0 / self.aspect - 1.0) * k)) };
                (u2 * u2 + v2 * v2).sqrt()
            } else if self.vig_round < 0.0 {
                let x = -self.vig_round;
                let p = 2.0 + VIG_ROUND_P[0] * x + VIG_ROUND_P[1] * x * x;
                let scale = 1.0 + VIG_ROUND_SCALE[0] * x + VIG_ROUND_SCALE[1] * x * x;
                (u.abs().powf(p) + v.abs().powf(p)).powf(1.0 / p) / scale
            } else {
                (u * u + v * v).sqrt()
            };
            let f = ramp(r, self.vig_centre - self.vig_half, self.vig_centre + self.vig_half).max(1e-6).powf(self.vig_gamma);
            let s = (self.vignette.abs() / 100.0).min(0.999);
            let keep = (1.0 - s * f).max(1e-3);
            let lum = (0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]).max(1e-6);
            if self.vignette < 0.0 {
                a += VIG_K * keep.log2() * (1.0 - vig_ease(lum, self.vig_highlights));
            } else {
                let y = lum.min(0.999);
                a += ((1.0 - (1.0 - y) * keep.powf(VIG_K_PLUS)) / y).log2();
            }
        }
        // Adaptive vignette: darkens around the subject's outline.
        if self.adaptive != 0.0 {
            if let Some(map) = &self.vig_map {
                a += self.adaptive / 100.0 * t.vig_amount * map.at(u, v);
            }
        }
        if a != 0.0 {
            c = c.map(|x| x * 2f32.powf(a));
        }
        c
    }
}

/// The parametric (region) curve, or None at its defaults: Lightroom's, as
/// measured (`paramtable`), or the rough fallback shape if the table is missing.
pub fn parametric_curve(s: &ToolSettings) -> Option<Curve> {
    let amounts = [s.param_shadows, s.param_darks, s.param_lights, s.param_highlights];
    if amounts.iter().all(|&a| a == 0.0) {
        return None;
    }
    let splits = [s.param_shadow_split, s.param_mid_split, s.param_high_split];
    Some(match crate::paramtable::table() {
        Some(t) => Curve::from_fn(|x| t.eval(x, amounts, splits)),
        None => Curve::from_fn(parametric_shape(amounts, splits)),
    })
}

/// The parametric curve sampled at `n` even points (0..1, gamma-2.2), for the
/// panel to draw.
pub fn parametric_points(amounts: [f32; 4], splits: [f32; 3], n: usize) -> Vec<f32> {
    let f: Box<dyn Fn(f32) -> f32> = match crate::paramtable::table() {
        Some(t) => Box::new(move |x| t.eval(x, amounts, splits)),
        None => Box::new(parametric_shape(amounts, splits)),
    };
    (0..n).map(|i| f(i as f32 / (n - 1).max(1) as f32)).collect()
}

/// A rough fallback shape (a smooth bump per region between its splits),
/// used only if the measured table is missing; also drawn by the panel until
/// the engine's curve arrives (`src/curves.ts`).
pub fn parametric_shape(amounts: [f32; 4], splits: [f32; 3]) -> impl Fn(f32) -> f32 {
    let [s1, m, s3] = splits.map(|v| (v / 100.0).clamp(0.05, 0.95));
    // A sin^2 bump rising from a to its peak and falling to b.
    let bump = move |x: f32, a: f32, peak: f32, b: f32| -> f32 {
        if x <= a || x >= b {
            return 0.0;
        }
        let t = if x < peak { 0.5 * (x - a) / (peak - a).max(1e-4) } else { 0.5 + 0.5 * (x - peak) / (b - peak).max(1e-4) };
        (std::f32::consts::PI * t).sin().powi(2)
    };
    let regions = [(0.0, s1, m, 0.15), (0.0, 0.5 * (s1 + m), s3, 0.2), (s1, 0.5 * (m + s3), 1.0, 0.2), (m, s3, 1.0, 0.15)];
    move |x: f32| {
        let mut y = x;
        for (k, &(a, peak, b, gain)) in regions.iter().enumerate() {
            y += amounts[k] / 100.0 * gain * bump(x, a, peak, b);
        }
        y.clamp(0.0, 1.0)
    }
}

/// The encoding the curves work on: a power `g` with an sRGB-style straight
/// toe (offset `a`; 0 = pure power, sRGB itself is 2.4 / 0.055).
#[derive(Clone, Copy)]
pub(crate) struct Encoding {
    pub(crate) g: f32,
    pub(crate) a: f32,
    pub(crate) x0: f32,
    pub(crate) slope: f32,
}

impl Encoding {
    pub(crate) fn new(g: f32, a: f32) -> Self {
        if a <= 0.0 {
            return Self { g, a: 0.0, x0: 0.0, slope: 0.0 };
        }
        let x0 = (a * g / ((1.0 + a) * (g - 1.0))).powf(g);
        let slope = ((1.0 + a) * x0.powf(1.0 / g) - a) / x0;
        Self { g, a, x0, slope }
    }

    fn encode(&self, x: f32) -> f32 {
        if x < self.x0 {
            x * self.slope
        } else {
            (1.0 + self.a) * x.powf(1.0 / self.g) - self.a
        }
    }

    fn decode(&self, y: f32) -> f32 {
        if y < self.x0 * self.slope {
            y / self.slope
        } else {
            ((y + self.a) / (1.0 + self.a)).max(0.0).powf(self.g)
        }
    }
}

/// Linear Rec.2020 -> linear sRGB.
fn rec2020_to_srgb() -> Mat3 {
    mul3(&crate::pipeline::XYZ_TO_SRGB, &crate::pipeline::REC2020_TO_XYZ)
}

/// Hue as Lightroom's bands see it: on gamma-encoded sRGB values (the
/// band centres - red 0, orange 30, yellow 60 ... - are defined there).
fn display_hue(c: [f32; 3], to_srgb: &Mat3) -> Option<f32> {
    let m = to_srgb;
    let s: [f32; 3] = std::array::from_fn(|i| (m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2]).clamp(0.0, 1.0).powf(1.0 / 2.2));
    rgb_hue(s)
}

/// The OKLab (a, b) direction Grade pushes for a Lightroom hue (degrees on
/// an HSV wheel): `space` 0 takes the wheel on gamma-encoded sRGB, 1 on
/// ProPhoto (Lightroom's working space, gamma 1.8), in between a blend.
fn hue_direction(hue: f32, space: f32, ok: &Oklab) -> [f32; 2] {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let pure = match h as i32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    };
    let dir = |rec2020: [f32; 3]| {
        let lab = ok.to_lab(rec2020.map(|v| v.max(0.0)));
        let n = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt().max(1e-6);
        [lab[1] / n, lab[2] / n]
    };
    let srgb = dir(apply3(&crate::whitebalance::invert3(&rec2020_to_srgb()), pure.map(|v: f32| v.powf(2.2))));
    if space <= 0.0 {
        return srgb;
    }
    // Half-saturated, so the direction isn't skewed by ProPhoto's
    // imaginary primaries.
    let soft = pure.map(|v: f32| (0.5 + 0.5 * v).powf(1.8));
    let pp = dir(apply3(&crate::whitebalance::invert3(&rec2020_to_prophoto()), soft));
    let d = [srgb[0] + (pp[0] - srgb[0]) * space, srgb[1] + (pp[1] - srgb[1]) * space];
    let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-6);
    [d[0] / n, d[1] / n]
}

/// RGB (0..1) -> HSV (hue degrees, saturation 0..1, value).
fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    [rgb_hue(c).unwrap_or(0.0), if max > 0.0 { (max - min) / max } else { 0.0 }, max]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let f = h - h.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match h.floor() as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn rgb_to_hsl(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d <= 1e-6 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6) };
    [rgb_hue(c).unwrap_or(0.0), s.min(1.0), l]
}

fn hsl_to_rgb([h, s, l]: [f32; 3]) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp.floor() as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

/// The Mixer as Lightroom appears to run it: bands and hue turns on a
/// ProPhoto RGB colour wheel (gamma-encoded), luminance as an exposure-like
/// gain that grows with saturation, then saturation in OKLab.
/// The chart's saturation and brightness levels (`examples/chartkit.rs`).
pub(crate) const SURF_S: [f32; 8] = [0.08, 0.16, 0.28, 0.42, 0.58, 0.74, 0.88, 1.0];
pub(crate) const SURF_V: [f32; 4] = [0.25, 0.45, 0.65, 0.85];

/// A Luminance correction surface read bilinearly at saturation `s` and value
/// `v` (held at the edges).
fn lum_surface(tab: &[[f32; 8]; 4], s: f32, v: f32) -> f32 {
    let pos = |levels: &[f32], x: f32| -> (usize, f32) {
        let last = levels.len() - 1;
        if x <= levels[0] {
            return (0, 0.0);
        }
        if x >= levels[last] {
            return (last - 1, 1.0);
        }
        let i = levels.windows(2).position(|w| x < w[1]).unwrap_or(last - 1);
        (i, (x - levels[i]) / (levels[i + 1] - levels[i]))
    };
    let ((si, sf), (vi, vf)) = (pos(&SURF_S, s), pos(&SURF_V, v));
    let row = |r: usize| tab[r][si] + (tab[r][si + 1] - tab[r][si]) * sf;
    row(vi) + (row(vi + 1) - row(vi)) * vf
}

fn mixer_prophoto(c: [f32; 3], mix: &[[f32; 3]; 8], t: &ToolTuning, ok: &Oklab, to_pp: &Mat3, from_pp: &Mat3, to_srgb: &Mat3) -> [f32; 3] {
    const G: f32 = 1.8; // ProPhoto's own encoding gamma
    let e = apply3(to_pp, c).map(|x| x.clamp(0.0, 1.0).powf(1.0 / G));
    let [h, s, v] = rgb_to_hsv(e);
    if s <= 1e-4 {
        return c;
    }
    let w = band_weights(h, t.mix_width, &t.mix_centre);
    // The pixel's saturation and value for the correction surfaces: on
    // ProPhoto (`mix_surface_pp`, where colours beyond sRGB keep their place)
    // or on gamma-encoded sRGB.
    let (cs, cv) = if t.mix_surface_pp > 0.5 {
        (s, v)
    } else {
        let e = apply3(to_srgb, c).map(|x| crate::pipeline::srgb_encode(x.clamp(0.0, 1.0)));
        let [_, s, v] = rgb_to_hsv(e);
        (s, v)
    };

    // Each band's own Luminance factor by the pixel's value (`SURF_V` levels).
    let by_value = |tab: &[f32; 4]| {
        let x = cv.clamp(SURF_V[0], SURF_V[3]);
        let i = SURF_V.windows(2).position(|p| x <= p[1]).unwrap_or(2);
        tab[i] + (tab[i + 1] - tab[i]) * (x - SURF_V[i]) / (SURF_V[i + 1] - SURF_V[i])
    };
    let (mut dh, mut ds, mut dl_up, mut dl_down) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for b in 0..8 {
        if w[b] == 0.0 {
            continue;
        }
        let [bh, bs, bl] = mix[b];
        dh += w[b] * bh / 100.0 * t.mix_hue[b] * if bh < 0.0 { t.mix_hue_neg[b] } else { 1.0 };
        // Saturation + builds up slower than linear (+50 is about half of
        // +100's effect in Lightroom): `mix_sat_curve` shapes it.
        ds += w[b] * if bs >= 0.0 { (bs / 100.0).powf(t.mix_sat_curve) * t.mix_sat_plus } else { bs / 100.0 * t.mix_sat_minus };
        let l = w[b] * bl / 100.0 * t.mix_lum_band[b];
        if bl >= 0.0 {
            dl_up += l * t.mix_lum_plus * by_value(&t.mix_lum_band_v_up[b]);
        } else {
            dl_down += l * t.mix_lum_minus * by_value(&t.mix_lum_band_v_down[b]);
        }
    }
    let amount = (s / t.mix_chroma_full.max(1e-3)).min(1.0);
    // (A Hue surface fitted the same way didn't help unseen photos.)
    if ds != 0.0 {
        ds *= lum_surface(if ds > 0.0 { &t.mix_sat_surface_up } else { &t.mix_sat_surface_down }, cs, cv);
        // Cuts bite harder on strong colours (`mix_sat_strong`).
        if ds < 0.0 {
            ds *= 1.0 + t.mix_sat_strong * s * s + t.mix_sat_bright * s * v.powi(4);
        }
    }
    // A Luminance lift pales bright strong colours too (`mix_lum_pale`).
    if dl_up > 0.0 && t.mix_lum_pale != 0.0 {
        ds -= t.mix_lum_pale * dl_up * s * v.powi(4);
    }
    let mut turned = if dh != 0.0 {
        apply3(from_pp, hsv_to_rgb([h + dh * amount, s, v]).map(|x| x.max(0.0).powf(G))).map(|x| x.max(0.0))
    } else {
        c
    };
    if dl_up != 0.0 || dl_down != 0.0 {
        // Luminance is HSL lightness on the ProPhoto wheel, scaled by a
        // factor (the same ratio at any brightness, as measured) with HSL
        // saturation held: brightening pales colours towards white,
        // darkening deepens them, and pale bright colours - high HSL
        // saturation - move a lot. `dl_*` are stops at full saturation,
        // `floor` the share a faint colour gets. `mix_lum_hsl` blends from
        // a plain exposure-like gain (0) to this (1).
        let e3 = apply3(to_pp, turned).map(|x| x.clamp(0.0, 1.0).powf(1.0 / G));
        let [hh, ss, ll] = rgb_to_hsl(e3);
        let lin = apply3(to_pp, c);
        let (hi, lo) = (lin[0].max(lin[1]).max(lin[2]), lin[0].min(lin[1]).min(lin[2]).max(0.0));
        let hsv_sat = if hi > 1e-6 { 1.0 - (lo / hi).powf(1.0 / G) } else { 0.0 };
        let sat = hsv_sat + (ss - hsv_sat) * t.mix_lum_hsl;
        let r = (sat / t.mix_lum_sat_full.max(1e-3)).min(1.0).powf(t.mix_lum_sat_pow);
        let up = t.mix_lum_floor + (1.0 - t.mix_lum_floor) * r;
        let down = t.mix_lum_floor_down + (1.0 - t.mix_lum_floor_down) * r;
        // Lightroom barely moves the darkest colours: fade in by lightness.
        let fade = if t.mix_lum_dark > 0.0 { ramp(ll, 0.0, t.mix_lum_dark) } else { 1.0 };
        // Lightroom's Luminance grows steeply with brightness and saturation
        // (measured on the synthetic chart): a correction surface by the
        // pixel's HSV saturation and value on gamma-encoded sRGB.
        let (su, sd) = (lum_surface(&t.mix_lum_surface_up, cs, cv), lum_surface(&t.mix_lum_surface_down, cs, cv));
        let stops = (dl_up * up * su + dl_down * down * sd) * fade;
        let gained = turned.map(|x| x * 2f32.powf(stops));
        if t.mix_lum_hsl <= 0.0 {
            turned = gained;
        } else {
            let lit = hsl_to_rgb([hh, ss, (ll * 2f32.powf(stops / G)).min(1.0)]);
            let hsl = apply3(from_pp, lit.map(|x| x.max(0.0).powf(G))).map(|x| x.max(0.0));
            // Strong colours pale more as they lighten (`mix_lum_hsl_strong`).
            let blend = (t.mix_lum_hsl + (1.0 - t.mix_lum_hsl) * t.mix_lum_hsl_strong * s * s).clamp(0.0, 1.0);
            turned = std::array::from_fn(|i| gained[i] + (hsl[i] - gained[i]) * blend);
        }
    }
    if ds == 0.0 {
        return turned;
    }
    let lab = ok.to_lab(turned);
    let k = (1.0 + ds * amount).max(0.0);
    let oklab = ok.to_rgb([lab[0], lab[1] * k, lab[2] * k]).map(|x| x.max(0.0));
    if t.mix_sat_hsl <= 0.0 {
        return oklab;
    }
    // Lightroom's saturation also moves brightness (more saturated blue is
    // darker, desaturated yellow too): HSL saturation scaled on the ProPhoto
    // wheel with HSL lightness held. Colours above 1 are scaled into range
    // and back so highlights aren't clipped.
    let lin = apply3(to_pp, turned).map(|x| x.max(0.0));
    let top = lin[0].max(lin[1]).max(lin[2]).max(1.0);
    let [hh, ss, ll] = rgb_to_hsl(lin.map(|x| (x / top).powf(1.0 / G)));
    // Raising: weak colours gain most, strong ones level off near full
    // (`mix_sat_soft` blends a plain scale into 1 - (1 - S)^k).
    let soft = |s: f32| if k > 1.0 { let plain = (s * k).min(1.0); plain + (1.0 - (1.0 - s).powf(k) - plain) * t.mix_sat_soft } else { s * k };
    let s2 = soft(ss);
    let mut hsl = apply3(from_pp, hsl_to_rgb([hh, s2, ll]).map(|x| x.max(0.0).powf(G) * top)).map(|x| x.max(0.0));
    if k > 1.0 && t.mix_sat_hsv > 0.0 {
        // Raising with the brightest channel held (HSV value): saturating
        // then always darkens a little, as Lightroom's does.
        let [h3, s3, v3] = rgb_to_hsv(lin.map(|x| (x / top).powf(1.0 / t.mix_sat_gamma)));
        let hsv = apply3(from_pp, hsv_to_rgb([h3, soft(s3), v3]).map(|x| x.max(0.0).powf(t.mix_sat_gamma) * top)).map(|x| x.max(0.0));
        hsl = std::array::from_fn(|i| hsl[i] + (hsv[i] - hsl[i]) * t.mix_sat_hsv);
    }
    std::array::from_fn(|i| oklab[i] + (hsl[i] - oklab[i]) * t.mix_sat_hsl)
}

fn mixer(c: [f32; 3], mix: &[[f32; 3]; 8], t: &ToolTuning, ok: &Oklab, to_srgb: &Mat3) -> [f32; 3] {
    let Some(hue) = display_hue(c, to_srgb) else { return c };
    let w = band_weights(hue, t.mix_width, &t.mix_centre);
    let (mut dh, mut ds, mut dl) = (0.0f32, 0.0f32, 0.0f32);
    for b in 0..8 {
        if w[b] == 0.0 {
            continue;
        }
        let [h, s, l] = mix[b];
        dh += w[b] * h / 100.0 * t.mix_hue[b];
        ds += w[b] * s / 100.0 * if s >= 0.0 { t.mix_sat_plus } else { t.mix_sat_minus };
        dl += w[b] * l / 100.0 * if l >= 0.0 { t.mix_lum_plus } else { t.mix_lum_minus };
    }
    let lab = ok.to_lab(c);
    let chroma = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    // Greys have no hue to adjust; weak colours get a partial effect.
    let amount = (chroma / t.mix_chroma_full.max(1e-3)).min(1.0);
    let angle = lab[2].atan2(lab[1]) + (dh * amount).to_radians();
    let l = (lab[0] + dl * amount).max(0.0);
    // As in Lightroom, darker colours get richer and lighter ones paler.
    let lum_rel = dl * amount / lab[0].max(0.05);
    let new_chroma = chroma * (1.0 + ds * amount).max(0.0) * (1.0 - t.mix_lum_chroma * lum_rel).max(0.0);
    let turned = ok.to_rgb([l, new_chroma * angle.cos(), new_chroma * angle.sin()]).map(|x| x.max(0.0));
    if dh == 0.0 || t.mix_hue_value == 0.0 {
        return turned;
    }
    // A hue turn in Lightroom changes brightness the way RGB "value" does
    // (aqua turned blue gets darker): blend towards keeping the brightest
    // channel where it was, measured on the hue-only turn.
    let hue_only = ok.to_rgb([lab[0], chroma * angle.cos(), chroma * angle.sin()]);
    let v_before = c[0].max(c[1]).max(c[2]);
    let v_after = hue_only[0].max(hue_only[1]).max(hue_only[2]);
    if v_before <= 1e-6 || v_after <= 1e-6 {
        return turned;
    }
    let k = (v_before / v_after).powf(t.mix_hue_value);
    turned.map(|x| x * k)
}

/// Grade, measured from Lightroom: each range shifts colours by about the
/// same amount in a perceptual colour space (OKLab) towards its hue - not by
/// adding to RGB, which hits strong colours differently - tapering off
/// towards black and white; luminance is a gain. `dirs` are the four
/// ranges' push directions (see `hue_direction`).
/// A range's measured strength at a lightness: linear between the five
/// points of `knots` (at lightness 0.1, 0.3, 0.5, 0.7, 0.9), flat beyond.
fn range_strength(knots: &[f32; 5], l: f32) -> f32 {
    let x = ((l - 0.1) / 0.2).clamp(0.0, 4.0);
    let i = (x.floor() as usize).min(3);
    let f = x - i as f32;
    (knots[i] + (knots[i + 1] - knots[i]) * f).max(0.0)
}

/// Where a Grade hue falls on the 30-degree setting tables: (node, next node, fraction).
pub fn set_hue_pos(hue: f32) -> (usize, usize, f32) {
    let x = hue.rem_euclid(360.0) / 30.0;
    let i = (x.floor() as usize) % 12;
    (i, (i + 1) % 12, x - x.floor())
}

/// Where a Saturation falls on the setting tables' 25 / 50 / 100 nodes.
pub fn set_sat_pos(sat: f32) -> (usize, f32) {
    let s = sat.abs();
    if s <= 25.0 {
        (0, 0.0)
    } else if s <= 50.0 {
        (0, (s - 25.0) / 25.0)
    } else {
        (1, ((s - 50.0) / 50.0).min(1.0))
    }
}

/// A per-hue table read at `hue`, interpolating around the wheel.
fn set_hue_at(tab: &[f32; 12], hue: f32) -> f32 {
    let (i, j, f) = set_hue_pos(hue);
    tab[i] + (tab[j] - tab[i]) * f
}

/// The setting gain table read at `hue` and `sat`.
fn set_gain_at(tab: &[[f32; 3]; 12], hue: f32, sat: f32) -> f32 {
    let (i, j, f) = set_hue_pos(hue);
    let (s, sf) = set_sat_pos(sat);
    let at = |n: usize| tab[n][s] + (tab[n][s + 1] - tab[n][s]) * sf;
    at(i) + (at(j) - at(i)) * f
}

/// What `grade` works out from the settings alone (Balance's and Blending's powers, each range's
/// peak, and for each tinted range its turned direction, gains and colours), once per render: it was
/// worked out again for every pixel. `grade_prepared` then does the same sums as before.
/// The grade's Shadows and Highlights tints alone (with Blending and Balance): the fitted model's
/// part when the measured table does the rest.
fn fitted_grade(gr: &GradeSettings) -> GradeSettings {
    let tint_only = |g: &GradeRange| GradeRange { lum: 0.0, ..*g };
    let none = GradeRange { sat: 0.0, lum: 0.0, ..gr.mids };
    GradeSettings { shadows: tint_only(&gr.shadows), highs: tint_only(&gr.highs), mids: none, global: GradeRange { sat: 0.0, lum: 0.0, ..gr.global }, ..*gr }
}

pub(crate) struct GradePrep {
    pub(crate) g: GradeSettings,
    /// 2 ^ (-Balance / 100 x `grade_balance`).
    pub(crate) bend: f32,
    /// Blending's power on each range's reach.
    pub(crate) e: f32,
    pub(crate) ref_e: [f32; 3],
    pub(crate) peaks: [f32; 4],
    pub(crate) tints: [Option<TintPrep>; 4],
}

pub(crate) struct TintPrep {
    pub(crate) dir: [f32; 2],
    pub(crate) light: f32,
    /// Saturation / 100 x `grade_sat` x the hue's and the table's gains (the start of `k_unit`).
    pub(crate) k_pre: f32,
    pub(crate) sat_shape: f32,
    pub(crate) pushed: [f32; 3],
    pub(crate) pure: [f32; 3],
    pub(crate) mean: f32,
}

impl GradePrep {
    fn new(g: &GradeSettings, t: &ToolTuning, ok: &Oklab, dirs: &[[f32; 2]; 4], spaces: [&Mat3; 4]) -> GradePrep {
        let to_srgb = if t.grade_rgb_pp > 0.5 { spaces[2] } else { spaces[0] };
        let gg = t.grade_rgb_gamma;
        let enc = |x: f32| x.signum() * x.abs().powf(1.0 / gg);
        let rgb_mode = t.grade_rgb > 0.0;
        let ranges = [g.shadows, g.mids, g.highs, g.global];
        let peaks: [f32; 4] = std::array::from_fn(|ri| t.grade_curves[ri].iter().cloned().fold(1e-6, f32::max));
        let tints: [Option<TintPrep>; 4] = std::array::from_fn(|ri| {
            let r = &ranges[ri];
            if r.sat == 0.0 {
                return None;
            }
            let dir = &dirs[ri];
            // The measured corrections for this range's settings.
            let turn = set_hue_at(&t.grade_set_turn[ri], r.hue);
            let (ts, tc) = turn.to_radians().sin_cos();
            let dir = [dir[0] * tc - dir[1] * ts, dir[0] * ts + dir[1] * tc];
            let table_gain = set_gain_at(&t.grade_set_gain[ri], r.hue, r.sat).max(0.0);
            let light = set_hue_at(&t.grade_set_light[ri], r.hue);
            let hr = r.hue.to_radians();
            let [g1, g2, g3, g4] = t.grade_hue_gain;
            let by_hue = (1.0 + g1 * hr.cos() + g2 * hr.sin() + g3 * (2.0 * hr).cos() + g4 * (2.0 * hr).sin()).max(0.0);
            let k_pre = r.sat / 100.0 * t.grade_sat * by_hue * table_gain;
            let sat_shape = (r.sat.abs() / 100.0).powf(t.grade_lift_satpow);
            let (pushed, pure, mean) = if rgb_mode {
                let pushed = apply3(to_srgb, ok.to_rgb([0.6, dir[0] * 0.05, dir[1] * 0.05])).map(enc);
                let pure = hsv_to_rgb([r.hue + t.grade_pure_offset + turn, 1.0, 1.0]);
                (pushed, pure, (pure[0] + pure[1] + pure[2]) / 3.0)
            } else {
                ([0.0; 3], [0.0; 3], 0.0)
            };
            Some(TintPrep { dir, light, k_pre, sat_shape, pushed, pure, mean })
        });
        GradePrep {
            g: *g,
            bend: 2f32.powf(-g.balance / 100.0 * t.grade_balance),
            e: t.grade_blend.max(0.1).powf((50.0 - g.blending) / 50.0),
            ref_e: if rgb_mode { apply3(to_srgb, ok.to_rgb([0.6, 0.0, 0.0])).map(enc) } else { [0.0; 3] },
            peaks,
            tints,
        }
    }
}

fn grade_prepared(c: [f32; 3], p: &GradePrep, t: &ToolTuning, ok: &Oklab, spaces: [&Mat3; 4]) -> [f32; 3] {
    let g = &p.g;
    // The RGB the tints are added to: sRGB, or ProPhoto with `grade_rgb_pp`.
    let (to_srgb, from_srgb) = if t.grade_rgb_pp > 0.5 { (spaces[2], spaces[3]) } else { (spaces[0], spaces[1]) };
    let mut lab = ok.to_lab(c.map(|x| x.max(0.0)));
    let chroma0 = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    // Lightness 0..1, bent by Balance (+ = more of the image counts as highlights).
    let v = lab[0].clamp(0.0, 1.0).powf(p.bend);
    // Blending sharpens (< 50) or softens (> 50) each range's reach.
    let e = p.e;
    // Pure black takes no tint.
    let black = ramp(lab[0], 0.0, t.grade_black);
    // Near white takes almost no tint either (Lightroom keeps whites white).
    let black = black * (1.0 - ramp(lab[0], t.grade_white, 1.0));
    let ranges = [g.shadows, g.mids, g.highs, g.global];
    // The tints' strength by the pixel's saturation and brightness (measured
    // on the synthetic chart), on gamma-encoded sRGB like the chart's patches.
    let tint_surface = {
        let e = apply3(spaces[0], c).map(|x| crate::pipeline::srgb_encode(x.clamp(0.0, 1.0)));
        let [_, s, val] = rgb_to_hsv(e);
        lum_surface(&t.grade_tint_surface, s, val)
    };
    // The RGB-additive alternative (`grade_rgb`): the same tints added to
    // gamma-encoded sRGB, so they fade near black and clip near white by
    // each pixel's own colour.
    let gg = t.grade_rgb_gamma;
    let enc = |x: f32| x.signum() * x.abs().powf(1.0 / gg);
    let dec = |x: f32| x.signum() * x.abs().powf(gg);
    let rgb_mode = t.grade_rgb > 0.0;
    let mut e_rgb = if rgb_mode { apply3(to_srgb, c).map(enc) } else { [0.0; 3] };
    let ref_e = p.ref_e;
    let l0 = lab[0];
    // Each channel's own level, bent by Balance, for `grade_tint_chan`.
    let bend = p.bend;
    let chan_v = e_rgb.map(|x| x.clamp(0.0, 1.0).powf(bend));
    // Each range's share of the tint here, for its lightness gains.
    let (mut k_sum, mut up, mut down) = (0.0f32, 0.0f32, 0.0f32);
    for (ri, curve) in t.grade_curves.iter().enumerate() {
        let peak = p.peaks[ri];
        let reach = |level: f32| peak * (range_strength(curve, level) / peak).powf(e) * black;
        let w = reach(v);
        if let Some(tp) = &p.tints[ri] {
            let dir = &tp.dir;
            let light = tp.light;
            // Already-strong colours take less tint (`grade_chroma_damp`); Lightroom's tints differ
            // in strength by hue (green and magenta strongest, yellow and blue weakest): in `k_pre`.
            let k_unit = tp.k_pre * tint_surface / (1.0 + chroma0 * t.grade_chroma_damp);
            let k = k_unit * w;
            k_sum += k.abs();
            // Lightroom's lightness shift grows faster than the tint itself
            // (about with Saturation squared: ~0 at 25, a quarter at 50).
            let sat_shape = tp.sat_shape;
            up += k.abs() * t.grade_lift_up[ri] * light * sat_shape;
            down += k.abs() * t.grade_lift_down[ri] * light * sat_shape;
            lab[1] += dir[0] * k;
            lab[2] += dir[1] * k;
            // Lightroom's tint also lifts lightness a little, by hue.
            lab[0] += k * (t.grade_lift[0] + t.grade_lift[1] * dir[0] + t.grade_lift[2] * dir[1]);
            if rgb_mode {
                // The tint's direction in encoded RGB, taken at a mid grey (`pushed`), or
                // (`grade_pure`) the pure colour of the hue itself, less its grey: yellow then
                // brightens, blue darkens, and green and magenta come out strongest, as measured.
                let (pushed, pure, mean) = (tp.pushed, tp.pure, tp.mean);
                for i in 0..3 {
                    let along = (pushed[i] - ref_e[i]) / 0.05;
                    let d = along + ((pure[i] - mean) * t.grade_pure_gain - along) * t.grade_pure;
                    // The channel's range weight: the pixel's, or its own level's.
                    let wi = if t.grade_tint_chan > 0.0 { w + (reach(chan_v[i]) - w) * t.grade_tint_chan } else { w };
                    e_rgb[i] += d * k_unit * wi * t.grade_rgb_gain;
                }
            }
        }
    }
    if rgb_mode {
        let rgb_tinted = ok.to_lab(apply3(from_srgb, e_rgb.map(|x| dec(x.clamp(0.0, 1.0)))).map(|x| x.max(0.0)));
        lab = std::array::from_fn(|i| lab[i] + (rgb_tinted[i] - lab[i]) * t.grade_rgb);
    }
    // The tints' lightness change, by range and by direction.
    if k_sum > 0.0 {
        let dl = lab[0] - l0;
        let gain = (if dl > 0.0 { up } else { down }) / k_sum;
        lab[0] = l0 + dl * gain;
    }
    // Luminance, like an exposure change on that range (colour strength
    // following it by a measured amount per range): a gain on light scales
    // OKLab by its cube root. Its own measured reach per range.
    let before_lum = lab;
    let (mut stops, mut colour_stops) = (0.0f32, 0.0f32);
    for ((r, curve), follow) in ranges.iter().zip(&t.grade_lum_curves).zip(t.grade_lum_chroma) {
        if r.lum != 0.0 {
            let s = r.lum / 100.0 * t.grade_lum * range_strength(curve, v) * black;
            stops += s;
            // How far colour strength follows: lifting shadows pales them in
            // Lightroom, lifting highlights enriches them.
            colour_stops += s * follow;
        }
    }
    if stops != 0.0 {
        let (k, kc) = (2f32.powf(stops / 3.0), 2f32.powf(colour_stops / 3.0));
        lab = [lab[0] * k, lab[1] * kc, lab[2] * kc];
        if t.grade_lum_rgb > 0.0 {
            // The alternative (`grade_lum_rgb`): the same gain per ProPhoto
            // channel, each by its own level, like an RGB tone curve - colours
            // then shift as channels move apart (most near white).
            let (to_pp, from_pp) = (spaces[2], spaces[3]);
            let bend = p.bend;
            let lin = apply3(to_pp, ok.to_rgb(before_lum).map(|x| x.max(0.0)));
            let moved: [f32; 3] = std::array::from_fn(|i| {
                let level = lin[i].clamp(0.0, 1.0).powf(1.0 / t.grade_lum_rgb_gamma.max(0.5)).powf(bend);
                let s: f32 = ranges
                    .iter()
                    .zip(&t.grade_lum_curves)
                    .filter(|(r, _)| r.lum != 0.0)
                    .map(|(r, curve)| r.lum / 100.0 * t.grade_lum * range_strength(curve, level) * black)
                    .sum();
                lin[i] * 2f32.powf(s)
            });
            let per_channel = ok.to_lab(apply3(from_pp, moved).map(|x| x.max(0.0)));
            lab = std::array::from_fn(|i| lab[i] + (per_channel[i] - lab[i]) * t.grade_lum_rgb);
        }
    }
    ok.to_rgb(lab).map(|x| x.max(0.0))
}

// ---- Definition: Texture, Clarity, Dehaze (need each pixel's neighbourhood) ----

pub fn definition_active(s: &ToolSettings) -> bool {
    s.texture != 0.0 || s.clarity != 0.0 || s.dehaze != 0.0
}

/// One box-blur pass of radius `r` along rows (`horizontal`) or columns,
/// with a running sum, so its cost doesn't grow with the radius.
fn box_pass(src: &[f32], w: usize, h: usize, r: usize, horizontal: bool) -> Vec<f32> {
    let mut out = vec![0.0f32; src.len()];
    box_pass_into(src, &mut out, w, h, r, horizontal);
    out
}

/// `box_pass` into `out` (as long as `src`; every value written).
fn box_pass_into(src: &[f32], out: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    use rayon::prelude::*;
    assert_eq!(out.len(), src.len());
    if !horizontal {
        return box_pass_down(src, out, w, h, r);
    }
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    // Each line written straight into place, on all cores (gathering them first and copying them
    // in on one core held a 24 MP Texture up by seconds, 24 Oct); the same sums in the same order.
    struct Out(*mut f32);
    unsafe impl Send for Out {}
    unsafe impl Sync for Out {}
    let dst = Out(out.as_mut_ptr());
    let dst = &dst;
    (0..lines).into_par_iter().for_each(|line| {
        let mut sum = 0.0f32;
        let clamp = |i: isize| src[at(line, i.clamp(0, len as isize - 1) as usize)];
        for i in -(r as isize)..=(r as isize) {
            sum += clamp(i);
        }
        let n = (2 * r + 1) as f32;
        for i in 0..len {
            // SAFETY: every (line, i) is a different index below src.len() = out.len(), and each
            // line is written by one task only, so no two tasks write the same element.
            unsafe { *dst.0.add(at(line, i)) = sum / n };
            sum += clamp(i as isize + r as isize + 1) - clamp(i as isize - r as isize);
        }
    });
}

/// `box_pass_into` down the columns, a band of columns at a time sweeping down the rows (reading
/// memory in order; walking each column alone missed the cache at every step). Each column keeps
/// its own running sum, added to in the same order as before, so the result is the same.
fn box_pass_down(src: &[f32], out: &mut [f32], w: usize, h: usize, r: usize) {
    use rayon::prelude::*;
    const BAND: usize = 256;
    struct Out(*mut f32);
    unsafe impl Send for Out {}
    unsafe impl Sync for Out {}
    let dst = Out(out.as_mut_ptr());
    let dst = &dst;
    let n = (2 * r + 1) as f32;
    let row = |i: isize| i.clamp(0, h as isize - 1) as usize * w;
    (0..w.div_ceil(BAND)).into_par_iter().for_each(|band| {
        let (x0, x1) = (band * BAND, ((band + 1) * BAND).min(w));
        let mut sum = vec![0.0f32; x1 - x0];
        for i in -(r as isize)..=(r as isize) {
            let base = row(i);
            for (s, x) in sum.iter_mut().zip(x0..x1) {
                *s += src[base + x];
            }
        }
        for y in 0..h {
            let (add, sub) = (row(y as isize + r as isize + 1), row(y as isize - r as isize));
            for (s, x) in sum.iter_mut().zip(x0..x1) {
                // SAFETY: (y, x) is inside the picture and in this band's columns only.
                unsafe { *dst.0.add(y * w + x) = *s / n };
                *s += src[add + x] - src[sub + x];
            }
        }
    });
}

/// A smooth blur: three box passes each way approximate a Gaussian.
fn blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    // Two buffers passed back and forth (six new full-size ones, each cleared by the system, cost
    // a 24 MP Texture more than the sums, 24 Oct); the same passes in the same order.
    let mut a = vec![0.0f32; src.len()];
    let mut b = vec![0.0f32; src.len()];
    box_pass_into(src, &mut a, w, h, r, true);
    box_pass_into(&a, &mut b, w, h, r, false);
    for _ in 1..3 {
        box_pass_into(&b, &mut a, w, h, r, true);
        box_pass_into(&a, &mut b, w, h, r, false);
    }
    b
}

/// OKLab a/b to CIELAB a*/b* sizes, roughly (a saturated red is a* 80 and
/// OKLab a 0.22), so Texture's colour settings read in familiar units.
pub(crate) const AB_UNITS: f32 = 360.0;

/// A blur at a fractional radius: the two nearest whole radii, blended.
fn blur_f(src: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    let lo = r.floor().max(0.0);
    let a = blur(src, w, h, lo as usize);
    let f = r - lo;
    if f < 0.02 {
        return a;
    }
    let b = blur(src, w, h, lo as usize + 1);
    use rayon::prelude::*;
    a.par_iter().zip(&b).map(|(x, y)| x + (y - x) * f).collect()
}

/// Single-pass box mean (radius `r`).
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let v = box_pass(src, w, h, r, true);
    box_pass(&v, w, h, r, false)
}

/// Guided filter (He et al.): smooths `input` while keeping the edges of `guide`.
pub fn guided_filter(guide: &[f32], input: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    use rayon::prelude::*;
    let mi = box_mean(guide, w, h, r);
    let mp = box_mean(input, w, h, r);
    let ii: Vec<f32> = guide.par_iter().map(|v| v * v).collect();
    let ip: Vec<f32> = guide.par_iter().zip(input).map(|(g, p)| g * p).collect();
    let mii = box_mean(&ii, w, h, r);
    drop(ii);
    let mip = box_mean(&ip, w, h, r);
    drop(ip);
    let a: Vec<f32> = (0..guide.len()).into_par_iter().map(|i| (mip[i] - mi[i] * mp[i]) / ((mii[i] - mi[i] * mi[i]).max(0.0) + eps)).collect();
    let b: Vec<f32> = (0..guide.len()).into_par_iter().map(|i| mp[i] - a[i] * mi[i]).collect();
    let ma = box_mean(&a, w, h, r);
    let mb = box_mean(&b, w, h, r);
    (0..guide.len()).into_par_iter().map(|i| ma[i] * guide[i] + mb[i]).collect()
}

/// Minimum over a (2r+1) square window (separable sliding minimum).
pub fn min_filter(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    use rayon::prelude::*;
    fn line_min(line: &[f32], r: usize) -> Vec<f32> {
        let n = line.len();
        let mut out = vec![0.0f32; n];
        let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        // Window for output i is [i - r, i + r], clamped.
        let mut next = 0usize;
        for i in 0..n {
            let hi = (i + r).min(n - 1);
            while next <= hi {
                while dq.back().is_some_and(|&b| line[b] >= line[next]) {
                    dq.pop_back();
                }
                dq.push_back(next);
                next += 1;
            }
            while dq.front().is_some_and(|&f| f + r < i) {
                dq.pop_front();
            }
            out[i] = line[*dq.front().unwrap()];
        }
        out
    }
    let rows: Vec<f32> = (0..h).into_par_iter().flat_map_iter(|y| line_min(&src[y * w..(y + 1) * w], r)).collect();
    // Each column's minimum written straight into place, on all cores (one core copied them in before).
    let mut out = vec![0.0f32; w * h];
    struct Out(*mut f32);
    unsafe impl Send for Out {}
    unsafe impl Sync for Out {}
    let dst = Out(out.as_mut_ptr());
    let dst = &dst;
    (0..w).into_par_iter().for_each(|x| {
        let col: Vec<f32> = (0..h).map(|y| rows[y * w + x]).collect();
        for (y, v) in line_min(&col, r).into_iter().enumerate() {
            // SAFETY: (y, x) is inside the picture, and column x is written by this task only.
            unsafe { *dst.0.add(y * w + x) = v };
        }
    });
    out
}

/// Dehaze on the display image (`d` = -1..1, Lightroom's slider / 100).
fn dehaze(
    px: &mut [[f32; 3]],
    dims: (usize, usize),
    d: f32,
    t: &ToolTuning,
    whole: Option<&DehazeMeasured>,
    frac: &(dyn Fn(usize) -> (f32, f32) + Sync),
) -> DehazeMeasured {
    use rayon::prelude::*;
    let g = t.dz_gamma;
    let enc = |v: f32| v.max(0.0).powf(1.0 / g);
    // A part of the photo uses the whole photo's measures and haze map.
    let m_ = match whole {
        Some(m) => m.clone(),
        None => dehaze_measure(px, dims, t),
    };
    let (nu, delta, av, mean_dc) = (m_.nu, m_.delta, m_.av, m_.mean_dc);
    let am = ((av[0] + av[1] + av[2]) / 3.0).max(1e-3);
    let ratio = av.map(|v| v / am);
    let m = d.abs();
    let smooth_at = |i: usize| -> f32 {
        let (fx, fy) = frac(i);
        m_.smooth.at(fx, fy)
    };
    let smooth: Vec<f32> = match (whole, &m_.full) {
        (None, Some(full)) => full.clone(),
        _ => (0..px.len()).into_par_iter().map(smooth_at).collect(),
    };
    if d < 0.0 {
        let k = (t.dz_m_amt * m.powf(t.dz_m_pow) * (1.0 + t.dz_m_med * (nu - 0.45) + t.dz_m_dark * (delta - 0.3))).clamp(0.0, 0.95);
        px.par_iter_mut().zip(smooth.par_iter()).for_each(|(c, &dcv)| {
            let kx = (k * (1.0 + t.dz_m_loc * (dcv / mean_dc - 1.0))).clamp(0.0, 0.95);
            let e = c.map(enc);
            *c = std::array::from_fn(|i| {
                let hz = (1.0 - t.dz_m_a) * t.dz_haze * (1.0 + t.dz_m_col * (ratio[i] - 1.0)) + t.dz_m_a * av[i];
                (e[i] * (1.0 - kx) + hz * kx).max(0.0).powf(g)
            });
        });
    } else {
        // Dark-channel dehaze: the more a region looks like the haze colour (all channels
        // bright against A), the less light it is taken to let through.
        let amount = t.dz_w * m.powf(t.dz_p_pow);
        // The haze taken off: A's colour against grey, by `dz_p_col`.
        let ap: [f32; 3] = std::array::from_fn(|i| am + t.dz_p_col * (av[i] - am));
        px.par_iter_mut().zip(smooth.par_iter()).for_each(|(c, &dcv)| {
            let tr = (1.0 - amount * dcv.clamp(0.0, 1.0)).clamp(t.dz_t0, 1.0);
            let e = c.map(enc);
            *c = std::array::from_fn(|i| (ap[i] + (e[i] - ap[i]) / tr).max(0.0).powf(g));
        });
    }
    DehazeMeasured { full: None, ..m_ }
}

/// Dehaze's measures of the whole photo: its brightness, the haze colour and
/// the smoothed dark-channel map.
pub(crate) fn dehaze_measure(px: &[[f32; 3]], dims: (usize, usize), t: &ToolTuning) -> DehazeMeasured {
    use rayon::prelude::*;
    let g = t.dz_gamma;
    let enc = |v: f32| v.max(0.0).powf(1.0 / g);
    // The photo's brightness: median encoded luminance, and mean darkest
    // channel, from every 7th pixel.
    // Each sample worked out on all cores, then added up in order (as before, so the same total).
    let samples: Vec<(f32, f32)> = px
        .par_iter()
        .step_by(7)
        .map(|c| {
            let e = c.map(enc);
            (LUMA[0] * e[0] + LUMA[1] * e[1] + LUMA[2] * e[2], e[0].min(e[1]).min(e[2]))
        })
        .collect();
    let mut lum: Vec<f32> = Vec::with_capacity(samples.len());
    let mut dark = 0.0f64;
    for &(l, d) in &samples {
        lum.push(l);
        dark += d as f64;
    }
    lum.par_sort_by(|a, b| a.total_cmp(b));
    let nu = lum[lum.len() / 2];
    let delta = (dark / lum.len() as f64) as f32;
    // Haze colour: the haziest pixels' average, as a ratio to its own mean.
    let (w_, h_) = dims;
    let dc: Vec<f32> = px.par_iter().map(|c| c.map(enc).iter().cloned().fold(f32::MAX, f32::min)).collect();
    let dc = blur(&dc, w_, h_, (w_.max(h_) / 60).max(1));
    let mut order: Vec<usize> = (0..px.len()).step_by(3).collect();
    // Stable, as `sort_by`: the same order.
    order.par_sort_by(|&a, &b| dc[b].total_cmp(&dc[a]));
    let take = ((order.len() as f32 * t.dz_a_frac) as usize).max(1);
    let mut av = [0.0f32; 3];
    for &i in &order[..take] {
        let e = px[i].map(enc);
        for k in 0..3 {
            av[k] += e[k] / take as f32;
        }
    }
    // The dark-channel map: how much each place looks like the haze colour.
    let long = w_.max(h_) as f32;
    let dcn: Vec<f32> = px.par_iter().map(|c| { let e = c.map(enc); (e[0] / av[0].max(1e-3)).min(e[1] / av[1].max(1e-3)).min(e[2] / av[2].max(1e-3)) }).collect();
    let eroded = min_filter(&dcn, w_, h_, ((t.dz_patch * long).round() as usize).max(1));
    drop(dcn);
    let smooth = {
        let r = (t.dz_smooth * long).max(1.0);
        let blurred = if t.dz_gf < 1.0 { blur_f(&eroded, w_, h_, r) } else { Vec::new() };
        if t.dz_gf > 0.0 {
            let guide: Vec<f32> = px.par_iter().map(|c| { let e = c.map(enc); LUMA[0] * e[0] + LUMA[1] * e[1] + LUMA[2] * e[2] }).collect();
            let g = guided_filter(&guide, &eroded, w_, h_, r.round() as usize, t.dz_eps);
            if t.dz_gf >= 1.0 { g } else { blurred.par_iter().zip(&g).map(|(b, g)| b + (g - b) * t.dz_gf).collect() }
        } else {
            blurred
        }
    };
    drop(eroded);
    let smooth: Vec<f32> = if (t.dz_dpow - 1.0).abs() > 1e-3 { smooth.par_iter().map(|v| v.clamp(0.0, 1.0).powf(t.dz_dpow)).collect() } else { smooth };
    let mean_dc = (smooth.iter().map(|&v| v as f64).sum::<f64>() / smooth.len().max(1) as f64).max(1e-3) as f32;
    DehazeMeasured { nu, delta, av, mean_dc, smooth: MapGrid::of(&smooth, w_, h_, 800), full: Some(smooth) }
}

// ---- Local Laplacian pyramid (after Paris et al. and Aubry et al.) ----

/// Half-size copy: 5-tap binomial blur, every other sample.
pub fn down2(src: &[f32], w: usize, h: usize) -> (Vec<f32>, usize, usize) {
    use rayon::prelude::*;
    let k = [1.0f32, 4.0, 6.0, 4.0, 1.0];
    let (nw, nh) = ((w + 1) / 2, (h + 1) / 2);
    // Horizontal pass on every row (kept at full height), then vertical on every other.
    let rows: Vec<f32> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let row = &src[y * w..(y + 1) * w];
            (0..nw).map(move |x| {
                let cx = 2 * x as isize;
                let mut s = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    s += kv * row[(cx + i as isize - 2).clamp(0, w as isize - 1) as usize];
                }
                s / 16.0
            })
        })
        .collect();
    let out: Vec<f32> = (0..nh)
        .into_par_iter()
        .flat_map_iter(|y| {
            let rows = &rows;
            (0..nw).map(move |x| {
                let cy = 2 * y as isize;
                let mut s = 0.0;
                for (j, kv) in k.iter().enumerate() {
                    s += kv * rows[((cy + j as isize - 2).clamp(0, h as isize - 1) as usize) * nw + x];
                }
                s / 16.0
            })
        })
        .collect();
    (out, nw, nh)
}

/// Bilinear enlargement of a (sw x sh) image to (w x h), which is about twice as big.
pub fn up2(src: &[f32], sw: usize, sh: usize, w: usize, h: usize) -> Vec<f32> {
    use rayon::prelude::*;
    (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..w).map(move |x| {
                let (fx, fy) = ((x as f32 + 0.5) / 2.0 - 0.5, (y as f32 + 0.5) / 2.0 - 0.5);
                let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
                let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
                let (tx, ty) = ((fx - x0 as f32).clamp(0.0, 1.0), (fy - y0 as f32).clamp(0.0, 1.0));
                let v = |xx: usize, yy: usize| src[yy.min(sh - 1) * sw + xx.min(sw - 1)];
                (v(x0, y0) * (1.0 - tx) + v(x1, y0) * tx) * (1.0 - ty) + (v(x0, y1) * (1.0 - tx) + v(x1, y1) * tx) * ty
            })
        })
        .collect()
}

pub type Level = (Vec<f32>, usize, usize);

pub fn gauss_pyramid(img: &[f32], w: usize, h: usize, levels: usize) -> Vec<Level> {
    let mut g: Vec<Level> = vec![(img.to_vec(), w, h)];
    for _ in 1..levels {
        let next = {
            let last = g.last().unwrap();
            down2(&last.0, last.1, last.2)
        };
        g.push(next);
    }
    g
}

/// Laplacian level `l` of a Gaussian pyramid (the top level is the residual itself).
pub fn laplacian_of(g: &[Level], l: usize) -> Vec<f32> {
    let (cur, w, h) = &g[l];
    if l + 1 == g.len() {
        return cur.clone();
    }
    let (next, w2, h2) = &g[l + 1];
    let up = up2(next, *w2, *h2, *w, *h);
    cur.iter().zip(&up).map(|(a, b)| a - b).collect()
}

/// The change local-Laplacian filtering makes to `img` (lightness 0-1),
/// with the change at pyramid level `l` weighted by `weights(l, levels)`.
pub fn local_laplacian_change(img: &[f32], w: usize, h: usize, sigma: f32, alpha: f32, refs: &[f32], weights: &dyn Fn(usize, usize) -> f32) -> Vec<f32> {
    local_laplacian_remap(img, w, h, refs, &|d: f32| d.signum() * sigma.powf(1.0 - alpha) * d.abs().powf(alpha), weights)
}

/// The same filter with an arbitrary remapping of the difference `d` = pixel - reference tone.
pub fn local_laplacian_remap(img: &[f32], w: usize, h: usize, refs: &[f32], remap: &(dyn Fn(f32) -> f32 + Sync), weights: &dyn Fn(usize, usize) -> f32) -> Vec<f32> {
    use rayon::prelude::*;
    let levels = (((w.min(h)) as f32).log2() as usize).saturating_sub(2).clamp(3, 9);
    let k = refs.len().max(2);
    let input = gauss_pyramid(img, w, h, levels);
    let in_lap: Vec<Vec<f32>> = (0..levels).map(|l| laplacian_of(&input, l)).collect();
    // For each reference tone, the Laplacian pyramid of the remapped image; each
    // output coefficient interpolates between the two references around the
    // input's own Gaussian value at that level.
    let zero: Vec<Vec<f32>> = input.iter().map(|(g, _, _)| vec![0.0f32; g.len()]).collect();
    // One reference at a time (each step is itself parallel), so only one
    // remapped pyramid is alive at once: at full resolution the all-at-once
    // version needed several GB.
    let mut diff_lap = zero;
    for ref_i in 0..k {
        let g = refs[ref_i];
        let remapped: Vec<f32> = img
            .par_iter()
            .map(|&v| g + remap(v - g))
            .collect();
        let pyr = gauss_pyramid(&remapped, w, h, levels);
        drop(remapped);
        for l in 0..levels {
            let lap = laplacian_of(&pyr, l);
            let gl = &input[l].0;
            diff_lap[l].par_iter_mut().zip(lap.par_iter()).zip(gl.par_iter()).for_each(|((acc, v), gv)| {
                // Hat weight of this reference at the input's value (references need not be evenly spaced).
                let x = *gv;
                let wt = if ref_i > 0 && x < refs[ref_i] && x >= refs[ref_i - 1] {
                    (x - refs[ref_i - 1]) / (refs[ref_i] - refs[ref_i - 1])
                } else if ref_i + 1 < k && x >= refs[ref_i] && x < refs[ref_i + 1] {
                    (refs[ref_i + 1] - x) / (refs[ref_i + 1] - refs[ref_i])
                } else if (ref_i == 0 && x < refs[0]) || (ref_i + 1 == k && x >= refs[k - 1]) {
                    1.0
                } else {
                    0.0
                };
                if wt > 0.0 {
                    *acc += wt * v;
                }
            });
        }
    }
    // Weighted change per level, collapsed from the top.
    let mut cur: Vec<f32> = diff_lap[levels - 1].iter().zip(&in_lap[levels - 1]).map(|(a, b)| weights(levels - 1, levels) * (a - b)).collect();
    for l in (0..levels - 1).rev() {
        let (_, w2, h2) = &input[l + 1];
        let (_, lw, lh) = &input[l];
        let up = up2(&cur, *w2, *h2, *lw, *lh);
        cur = (0..lw * lh).map(|p| up[p] + weights(l, levels) * (diff_lap[l][p] - in_lap[l][p])).collect();
    }
    cur
}

/// Learned Clarity+ lightness change (L* units, full slider) from `clarity_net`: features of the render itself
/// (lightness and its ratio to the local mean at eight scales, local spread, local means, the photo's histogram
/// quantiles, fine detail and spread of the blurred lightness), computed at about 1000 px like the training data
/// and upsampled. See examples/clarnn.rs for the training and the feature definitions.
pub(crate) struct NetWeights {
    mu: &'static [f32],
    sd: &'static [f32],
    w1: &'static [f32],
    b1: &'static [f32],
    w2: &'static [f32],
    b2: &'static [f32],
    w3: &'static [f32],
    b3: &'static [f32],
}

pub(crate) fn clarity_net_positive() -> NetWeights {
    use crate::clarity_net::*;
    NetWeights { mu: &MU, sd: &SD, w1: &W1, b1: &B1, w2: &W2, b2: &B2, w3: &W3, b3: &B3 }
}

pub(crate) fn clarity_net_negative() -> NetWeights {
    use crate::clarity_net_neg::*;
    NetWeights { mu: &MU, sd: &SD, w1: &W1, b1: &B1, w2: &W2, b2: &B2, w3: &W3, b3: &B3 }
}

/// The block size of Clarity's lightness network's small copy of a `w` x `h` picture.
pub(crate) fn clarity_net_factor(w: usize, h: usize) -> usize {
    ((w.max(h) as f32 / 1000.0).round() as usize).max(1)
}

fn clarity_net_change(px: &[[f32; 3]], w: usize, h: usize, net: &NetWeights) -> Vec<f32> {
    use rayon::prelude::*;
    let f = clarity_net_factor(w, h);
    let (lw, lh) = (w.div_ceil(f), h.div_ceil(f));
    let yrow = crate::pipeline::REC2020_TO_XYZ[1];
    // Luminance and CIE L* on the small grid (block average of linear light).
    let mut ysm = vec![0.0f32; lw * lh];
    for y in 0..lh {
        for x in 0..lw {
            let (mut s, mut n) = (0.0f32, 0.0f32);
            for yy in (y * f)..((y + 1) * f).min(h) {
                for xx in (x * f)..((x + 1) * f).min(w) {
                    let p = px[yy * w + xx];
                    s += (yrow[0] * p[0] + yrow[1] * p[1] + yrow[2] * p[2]).clamp(0.0, 1.0);
                    n += 1.0;
                }
            }
            ysm[y * lw + x] = s / n;
        }
    }
    let small = clarity_net_small(&ysm, lw, lh, net);
    if f == 1 {
        return small;
    }
    // Bilinear upsample to full size.
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (fx, fy) = ((x as f32 + 0.5) / f as f32 - 0.5, (y as f32 + 0.5) / f as f32 - 0.5);
            let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
            let (x1, y1) = ((x0 + 1).min(lw - 1), (y0 + 1).min(lh - 1));
            let (tx, ty) = ((fx - x0 as f32).clamp(0.0, 1.0), (fy - y0 as f32).clamp(0.0, 1.0));
            let a = small[y0 * lw + x0] * (1.0 - tx) + small[y0 * lw + x1] * tx;
            let b = small[y1 * lw + x0] * (1.0 - tx) + small[y1 * lw + x1] * tx;
            a * (1.0 - ty) + b * ty
        })
        .collect()
}

/// Clarity's lightness network on the small copy (`ysm`: block averages of the luminance, each
/// clamped to 0..1, `lw` x `lh`): its change in L* there.
pub(crate) fn clarity_net_small(ysm: &[f32], lw: usize, lh: usize, net: &NetWeights) -> Vec<f32> {
    use crate::clarity_net::NF;
    use rayon::prelude::*;
    let lstar: Vec<f32> = ysm.iter().map(|&y| if y > 0.008856 { 116.0 * y.cbrt() - 16.0 } else { 903.3 * y }).collect();
    let yp: Vec<f32> = lstar.iter().map(|&l| { let q = (l.clamp(0.0, 100.0) + 16.0) / 116.0; q * q * q + 1e-3 }).collect();
    let sq: Vec<f32> = lstar.iter().map(|l| l * l).collect();
    let integral = |v: &[f32]| -> Vec<f64> {
        let mut ii = vec![0.0f64; (lw + 1) * (lh + 1)];
        for y in 0..lh {
            let mut row = 0.0f64;
            for x in 0..lw {
                row += v[y * lw + x] as f64;
                ii[(y + 1) * (lw + 1) + x + 1] = ii[y * (lw + 1) + x + 1] + row;
            }
        }
        ii
    };
    let (iy, il, isq) = (integral(&yp), integral(&lstar), integral(&sq));
    let nf = net.mu.len();
    let mean_at = |ii: &[f64], x: usize, y: usize, r: usize| -> f64 {
        let (x0, x1, y0, y1) = (x.saturating_sub(r), (x + r + 1).min(lw), y.saturating_sub(r), (y + r + 1).min(lh));
        (ii[y1 * (lw + 1) + x1] - ii[y0 * (lw + 1) + x1] - ii[y1 * (lw + 1) + x0] + ii[y0 * (lw + 1) + x0]) / ((x1 - x0) * (y1 - y0)) as f64
    };
    let lg = lw.max(lh) as f32;
    let rad = |pct: f32| ((pct / 100.0) * lg).max(1.0) as usize;
    let rs: Vec<usize> = [0.3f32, 0.6, 1.2, 2.5, 5.0, 10.0, 20.0, 40.0].iter().map(|&q| rad(q)).collect();
    // Photo-level statistics.
    let mut ls: Vec<f32> = lstar.iter().step_by(7).copied().collect();
    ls.sort_by(|a, b| a.total_cmp(b));
    let q = |fr: f32| ls[((ls.len() - 1) as f32 * fr) as usize] / 100.0;
    let mean_l = ls.iter().sum::<f32>() / ls.len() as f32 / 100.0;
    let r37 = rad(3.7);
    let det = {
        let (mut s, mut n) = (0.0f64, 0.0f64);
        for y in (0..lh).step_by(5) {
            for x in (0..lw).step_by(5) {
                s += (lstar[y * lw + x] as f64 - mean_at(&il, x, y, r37)).abs();
                n += 1.0;
            }
        }
        (s / n.max(1.0) / 100.0) as f32
    };
    let spread = {
        let mut v: Vec<f32> = Vec::new();
        for y in (0..lh).step_by(9) {
            for x in (0..lw).step_by(9) {
                v.push(mean_at(&il, x, y, r37) as f32);
            }
        }
        v.sort_by(|a, b| a.total_cmp(b));
        (v[(v.len() - 1) * 95 / 100] - v[(v.len() - 1) * 5 / 100]) / 100.0
    };
    let photo = [mean_l, q(0.05), q(0.25), q(0.5), q(0.75), q(0.95), det, spread];
    let (r12, r5, r5m, r20m) = (rad(1.2), rad(5.0), rad(5.0), rad(20.0));
    // Edge-aware structure (only for nets that take them): guided-filter differences and local max/min ranges.
    let need_edge = nf > 36;
    let lnorm: Vec<f32> = if need_edge { lstar.iter().map(|l| l / 100.0).collect() } else { Vec::new() };
    let gf = |r: usize, eps: f32| if need_edge { guided_filter(&lnorm, &lnorm, lw, lh, r, eps) } else { Vec::new() };
    let (gf1, gf5, gf5s, gf20) = (gf(rad(1.2), 0.0025), gf(rad(5.0), 0.0025), gf(rad(5.0), 0.0004), gf(rad(20.0), 0.0025));
    let (mn5, mn20, mx5, mx20) = if need_edge {
        let neg: Vec<f32> = lnorm.iter().map(|v| -v).collect();
        (min_filter(&lnorm, lw, lh, rad(5.0)), min_filter(&lnorm, lw, lh, rad(20.0)), min_filter(&neg, lw, lh, rad(5.0)), min_filter(&neg, lw, lh, rad(20.0)))
    } else {
        (Vec::new(), Vec::new(), Vec::new(), Vec::new())
    };
    let small: Vec<f32> = (0..lw * lh)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % lw, i / lw);
            let mut inp = [0.0f32; NF];
            inp[0] = lstar[i] / 100.0;
            for (k, &r) in rs.iter().enumerate() {
                inp[1 + k] = (yp[i] as f64 / mean_at(&iy, x, y, r)).ln() as f32;
            }
            for (k, &r) in [r12, r5].iter().enumerate() {
                let (m, m2) = (mean_at(&il, x, y, r), mean_at(&isq, x, y, r));
                inp[9 + k] = ((m2 - m * m).max(0.0).sqrt() / 100.0) as f32;
            }
            inp[11] = (mean_at(&il, x, y, r5m) / 100.0) as f32;
            inp[12] = (mean_at(&il, x, y, r20m) / 100.0) as f32;
            // inp[13], inp[14] (chroma) are not used: zero, which standardises to zero.
            inp[15..23].copy_from_slice(&photo);
            if need_edge {
                let ln = lnorm[i];
                let (m5, m20) = (inp[11], inp[12]);
                inp[36] = ln - gf1[i];
                inp[37] = ln - gf5[i];
                inp[38] = ln - gf20[i];
                inp[39] = ln - gf5s[i];
                inp[40] = gf5[i] - m5;
                inp[41] = gf20[i] - m20;
                inp[42] = -mx5[i] - ln;
                inp[43] = ln - mn5[i];
                inp[44] = -mx20[i] - ln;
                inp[45] = ln - mn20[i];
            }
            // Two hidden layers (ReLU) and a linear output; inputs beyond the 23 used here stay zero.
            let mut x = [0.0f32; NF];
            for j in 0..nf {
                x[j] = (inp[j] - net.mu[j]) / net.sd[j];
            }
            let (n1, n2) = (net.b1.len(), net.b2.len());
            let mut h1 = vec![0.0f32; n1];
            for k in 0..n1 {
                let mut s = net.b1[k];
                for j in 0..nf {
                    s += net.w1[k * nf + j] * x[j];
                }
                h1[k] = s.max(0.0);
            }
            let mut out = net.b3[0];
            for k in 0..n2 {
                let mut s = net.b2[k];
                for j in 0..n1 {
                    s += net.w2[k * n1 + j] * h1[j];
                }
                out += net.w3[k] * s.max(0.0);
            }
            out
        })
        .collect();
    small
}

/// A low-resolution map over the whole photo, read at any place by fractions
/// (bilinear, cell centres), for rendering a part of it at full resolution.
#[derive(Clone, Debug, Default)]
pub struct MapGrid {
    pub v: Vec<f32>,
    pub w: usize,
    pub h: usize,
}

impl MapGrid {
    pub fn at(&self, fx: f32, fy: f32) -> f32 {
        if self.v.is_empty() {
            return 0.0;
        }
        let x = (fx * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (fy * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let a = self.v[y0 * self.w + x0] * (1.0 - tx) + self.v[y0 * self.w + x1] * tx;
        let b = self.v[y1 * self.w + x0] * (1.0 - tx) + self.v[y1 * self.w + x1] * tx;
        a * (1.0 - ty) + b * ty
    }

    /// A map of `v` (w x h), shrunk by block averages to at most `long` on the long side.
    pub fn of(v: &[f32], w: usize, h: usize, long: usize) -> MapGrid {
        let f = w.max(h).div_ceil(long.max(1)).max(1);
        let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
        let mut out = vec![0.0f32; sw * sh];
        for y in 0..sh {
            for x in 0..sw {
                let (mut s, mut n) = (0.0f32, 0.0f32);
                for yy in (y * f)..((y + 1) * f).min(h) {
                    for xx in (x * f)..((x + 1) * f).min(w) {
                        s += v[yy * w + xx];
                        n += 1.0;
                    }
                }
                out[y * sw + x] = s / n.max(1.0);
            }
        }
        MapGrid { v: out, w: sw, h: sh }
    }
}

/// What Dehaze measured on the whole photo.
#[derive(Clone, Debug, Default)]
pub struct DehazeMeasured {
    pub(crate) nu: f32,
    pub(crate) delta: f32,
    pub(crate) av: [f32; 3],
    pub(crate) mean_dc: f32,
    /// The smoothed dark-channel map (how hazy each place looks).
    pub(crate) smooth: MapGrid,
    /// The same map at full size, while rendering the photo it was measured on.
    pub(crate) full: Option<Vec<f32>>,
}

/// What Texture / Clarity / Dehaze measured on the whole photo (see
/// `apply_definition`), so a part rendered later matches.
#[derive(Clone, Debug, Default)]
pub struct DefinitionMeasured {
    /// Clarity's display network: log gain over the photo.
    pub(crate) clar_gain: Option<MapGrid>,
    /// Clarity's lightness network: its change in L* over the photo.
    pub(crate) clar_change: Option<MapGrid>,
    pub(crate) dehaze: Option<DehazeMeasured>,
    /// The masks' Texture / Clarity / Dehaze, each tool each way (tool x 2 + 0 up, 1 down): what
    /// each measured on the whole photo, so slider drags and zoomed-in parts reuse it as the
    /// sliders' do (render_core_q).
    pub(crate) masks: Vec<(usize, DefinitionMeasured)>,
}

impl DefinitionMeasured {
    /// What the masks' tool `key` measured, when it holds what `one` (that tool alone) needs.
    pub(crate) fn mask(&self, key: usize, one: &ToolSettings, t: &ToolTuning) -> Option<&DefinitionMeasured> {
        self.masks.iter().find(|(k, _)| *k == key).map(|(_, m)| m).filter(|m| m.covers(one, t))
    }

    /// Whether it holds what these settings' Clarity and Dehaze need.
    pub fn covers(&self, s: &ToolSettings, t: &ToolTuning) -> bool {
        let clarity_ok = s.clarity == 0.0
            || (s.clarity > 0.0 && t.clar_disp > 0.5 && self.clar_gain.is_some())
            || (s.clarity > 0.0 && t.clar_disp <= 0.5 && self.clar_change.is_some())
            || (s.clarity < 0.0 && self.clar_change.is_some());
        let dehaze_ok = s.dehaze == 0.0 || self.dehaze.is_some();
        clarity_ok && dehaze_ok
    }
}

/// Rendering a part of the photo: where it sits, and what was measured on the
/// whole photo. Both None for the whole photo.
#[derive(Clone, Copy, Default)]
pub struct DefinitionPart<'a> {
    pub frame: Option<&'a crate::pipeline::Frame>,
    pub measured: Option<&'a DefinitionMeasured>,
}

/// Clarity's lightness network's change (L*): computed on this image, or for
/// a part of the photo read off the whole photo's (measured with it).
fn net_change_part(
    px: &[[f32; 3]],
    w: usize,
    h: usize,
    net: &NetWeights,
    part: DefinitionPart,
    frac: &(dyn Fn(usize) -> (f32, f32) + Sync),
    measured: &mut DefinitionMeasured,
) -> Vec<f32> {
    use rayon::prelude::*;
    if let Some(grid) = part.measured.and_then(|m| m.clar_change.as_ref()) {
        return (0..w * h).into_par_iter().map(|i| { let (fx, fy) = frac(i); grid.at(fx, fy) }).collect();
    }
    let d = clarity_net_change(px, w, h, net);
    measured.clar_change = Some(MapGrid::of(&d, w, h, 1000));
    d
}

/// Texture, Clarity and Dehaze on the rendered (display-linear) image.
/// Texture and Clarity add back detail in OKLab lightness (the difference
/// from a blurred copy) at a small and a large radius, Clarity mostly in
/// the mid-tones. Dehaze works like haze itself, measured from Lightroom:
/// + takes the haze colour out (darker, and bluer under warm haze), - mixes
/// it in (brighter, washed out).
pub fn apply_definition(
    px: &mut [[f32; 3]],
    w: usize,
    h: usize,
    source_long: usize,
    s: &ToolSettings,
    t: &ToolTuning,
    ok: &Oklab,
    part: DefinitionPart,
) -> Option<DefinitionMeasured> {
    use rayon::prelude::*;
    let mut measured = DefinitionMeasured::default();
    // Pixel i as fractions of the whole photo (a part sits within it).
    let frac = |i: usize| match part.frame {
        Some(f) => f.frac(i, w),
        None => (((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32),
    };
    // Display-domain Clarity+: one luminance gain from the network, applied to red, green and blue alike, on the finished picture.
    let s_local;
    let mut s = s;
    if s.clarity > 0.0 && t.clar_disp > 0.5 {
        let y_row = crate::pipeline::REC2020_TO_XYZ[1];
        let lum: Vec<f32> = px.par_iter().map(|c| (y_row[0] * c[0] + y_row[1] * c[1] + y_row[2] * c[2]).max(0.0)).collect();
        let a = (s.clarity / 100.0).powf(t.clar_pow);
        let g = match part.measured.and_then(|m| m.clar_gain.as_ref()) {
            Some(grid) => (0..lum.len()).into_par_iter().map(|i| { let (fx, fy) = frac(i); grid.at(fx, fy) }).collect::<Vec<f32>>(),
            None => {
                let _p = crate::prof::span("    Clarity+ network");
                let g = crate::clarity_unet::gain_map(&lum, w, h);
                measured.clar_gain = Some(MapGrid::of(&g, w, h, crate::clarity_unet::NET_LONG));
                g
            }
        };
        // Calibration against Lightroom on held-out photos: overall strength, and a lift (L* at +100) per decile of display lightness.
        let amp: f32 = std::env::var("CLARDISP_AMP").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let tone: f32 = std::env::var("CLARDISP_TONE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);   // on: strict 58.5% vs 57.8%, zone means match Lightroom (150 held-out photos)
        px.par_iter_mut().zip(g.par_iter().zip(lum.par_iter())).for_each(|(c, (&gg, &y))| {
            let yy = y.clamp(0.0, 1.0);
            let l = if yy > 0.008856 { 116.0 * yy.cbrt() - 16.0 } else { 903.3 * yy };
            let k = (a * (gg * amp + tone * crate::clarity_unet::tone_correction(l) * 3.0 / (l + 16.0))).exp();
            *c = c.map(|v| v * k);
        });
        s_local = ToolSettings { clarity: 0.0, ..s.clone() };
        s = &s_local;
    }
    if s.dehaze != 0.0 {
        let _p = crate::prof::span("    Dehaze");
        let m = dehaze(px, (w, h), s.dehaze / 100.0, t, part.measured.and_then(|m| m.dehaze.as_ref()), &frac);
        if part.measured.is_none() {
            measured.dehaze = Some(m);
        }
    }
    let mut lab: Vec<[f32; 3]> = px.par_iter().map(|&c| ok.to_lab(c.map(|x| x.max(0.0)))).collect();
    let l: Vec<f32> = lab.par_iter().map(|p| p[0]).collect();
    // The whole photo's long side at this image's scale (a part's, too).
    let long = match part.frame {
        Some(f) => f.full_w.max(f.full_h),
        None => w.max(h) as f32,
    };
    // Pixels of this image per pixel of the full-resolution photo.
    let shrink = long / source_long.max(1) as f32;
    let mut new_l = l.clone();
    // Texture's change to lightness, a and b, added after Clarity's.
    let mut texture: Vec<[f32; 3]> = Vec::new();
    let p_tex = crate::prof::span("    Texture");
    if s.texture != 0.0 {
        let r = (t.tex_radius * shrink).max(1.0);
        let chan = |k: usize| -> Vec<f32> {
            let v: Vec<f32> = lab.par_iter().map(|p| p[k]).collect();
            let b = blur_f(&v, w, h, r);
            v.par_iter().zip(&b).map(|(x, y)| x - y).collect()
        };
        let (hl, ha, hb) = (chan(0), chan(1), chan(2));
        let a = (s.texture.abs() / 100.0).powf(t.tex_pow) * s.texture.signum() * if s.texture < 0.0 { t.tex_minus } else { 1.0 };
        let sm = |e0: f32, e1: f32, v: f32| {
            let u = ((v - e0) / (e1 - e0)).clamp(0.0, 1.0);
            u * u * (3.0 - 2.0 * u)
        };
        texture = (0..lab.len())
            .into_par_iter()
            .map(|i| {
                // Lightness detail and change in L* units (OKLab lightness x 100).
                let x = 100.0 * hl[i];
                let y = x.signum() * (1.0 + x.abs() / t.tex_knee).ln() * if x < 0.0 { t.tex_dark } else { 1.0 };
                let lstar = 100.0 * l[i];
                let tone_l = (1.0 - t.tex_tone_lo * (1.0 - sm(0.0, 25.0, lstar))) * (1.0 - t.tex_tone_hi * sm(75.0, 100.0, lstar));
                // Colour detail: a and b in CIELAB-sized units, nearly
                // linear, easing off for the strongest.
                let tone_c = 1.0 - t.tex_chroma_hi * sm(75.0, 100.0, lstar);
                let c = |h: f32| {
                    let x = AB_UNITS * h;
                    a * t.tex_chroma * tone_c * x / (1.0 + x.abs() / t.tex_chroma_knee) / AB_UNITS
                };
                [a * t.tex_amount * y * tone_l / 100.0, c(ha[i]), c(hb[i])]
            })
            .collect();
    }
    drop(p_tex);
    let _p_clar = crate::prof::span("    Clarity");
    if s.clarity > 0.0 && t.clar_net > 0.5 {
        let a = (s.clarity / 100.0).powf(t.clar_pow);
        let d = net_change_part(px, w, h, &clarity_net_positive(), part, &frac, &mut measured);
        // The net's change is in CIE L*; OKLab lightness of a grey is the cube root of its luminance, so 1 L* is ~1/116.
        new_l.par_iter_mut().zip(d.par_iter()).for_each(|(v, &c)| *v += a * c / 116.0);
    } else if s.clarity > 0.0 {
        let mut a = (s.clarity / 100.0).powf(t.clar_pow);
        if t.clar_pb != 0.0 || t.clar_pd != 0.0 {
            let base = blur_f(&l, w, h, (t.clar_fr * long).max(1.0));
            let mut v: Vec<f32> = base.iter().step_by(5).map(|x| x * 100.0).collect();
            v.sort_by(|a, b| a.total_cmp(b));
            let range = (v[(v.len() - 1) * 95 / 100] - v[(v.len() - 1) * 5 / 100]).max(1.0);
            let detail = (l.iter().zip(&base).step_by(5).map(|(x, b)| (x - b).abs() as f64).sum::<f64>() / (l.len() / 5).max(1) as f64) as f32 * 100.0;
            a *= (range / t.clar_rb).powf(t.clar_pb) * (detail.max(0.05) / t.clar_rd).powf(t.clar_pd);
        }
        let wts = |lv: usize, levels: usize| -> f32 {
            if lv + 1 == levels {
                t.clar_w[3]
            } else if lv < 2 {
                t.clar_w[0]
            } else if lv < 5 {
                t.clar_w[1]
            } else {
                t.clar_w[2]
            }
        };
        let change = local_laplacian_change(&l, w, h, t.clar_sigma, t.clar_alpha, &(0..12).map(|i| i as f32 / 11.0).collect::<Vec<f32>>(), &wts);
        let mut cdf = vec![0.0f32; 1025];
        for &v in l.iter() {
            cdf[((v.clamp(0.0, 1.0) * 1023.0) as usize) + 1] += 1.0;
        }
        for i in 1..cdf.len() {
            cdf[i] += cdf[i - 1];
        }
        let total = cdf[1024].max(1.0);
        new_l.par_iter_mut().enumerate().for_each(|(i, v)| {
            let mid = (4.0 * l[i] * (1.0 - l[i])).clamp(0.0, 1.0).powf(t.clar_mid);
            let eq = cdf[((l[i].clamp(0.0, 1.0) * 1023.0) as usize) + 1] / total - l[i];
            *v += a * (mid * change[i] + t.clar_eq * eq);
        });
    } else if s.clarity < 0.0 && t.clar_net > 0.5 {
        let a = (-s.clarity / 100.0).powf(t.clar_n_pow);
        let d = net_change_part(px, w, h, &clarity_net_negative(), part, &frac, &mut measured);
        new_l.par_iter_mut().zip(d.par_iter()).for_each(|(v, &c)| *v += a * c / 116.0);
    } else if s.clarity < 0.0 {
        let a = (-s.clarity / 100.0).powf(t.clar_n_pow) * t.clar_minus;
        let loc = blur_f(&l, w, h, (t.clar_n_radius * long).max(1.0));
        new_l.par_iter_mut().enumerate().for_each(|(i, v)| {
            let x = 100.0 * (l[i] - loc[i]);
            let pos = (l[i] * 100.0 / 100.0 * 100.0).clamp(0.0, 100.0);
            let nodes = [0.0f32, 15.0, 30.0, 45.0, 60.0, 75.0, 90.0, 100.0];
            let k = (0..7).find(|&k| pos <= nodes[k + 1]).unwrap_or(6);
            let f = ((pos - nodes[k]) / (nodes[k + 1] - nodes[k])).clamp(0.0, 1.0);
            let tone = t.clar_n_tone[k] * (1.0 - f) + t.clar_n_tone[k + 1] * f;
            *v -= a * t.clar_n_amount * tone * x / (1.0 + x.abs() / t.clar_n_knee) / 100.0;
        });
    }
    let d = s.dehaze / 100.0;
    lab.par_iter_mut().zip(new_l.par_iter()).for_each(|(p, &nl)| {
        let lv = nl;
        let chroma_k = if d > 0.0 { 1.0 + t.dehaze_sat * d } else { 1.0 };
        // Keep colour strength in step with lightness so detail doesn't
        // wash out or oversaturate.
        let scale = if p[0] > 1e-4 { (lv / p[0]).max(0.0) } else { 1.0 };
        let cs = scale.powf(t.clar_cexp);
        *p = [lv.max(0.0), p[1] * chroma_k * cs, p[2] * chroma_k * cs];
    });
    if !texture.is_empty() {
        lab.par_iter_mut().zip(texture.par_iter()).for_each(|(p, d)| *p = [(p[0] + d[0]).max(0.0), p[1] + d[1], p[2] + d[2]]);
    }
    px.par_iter_mut().zip(lab.par_iter()).for_each(|(c, p)| *c = ok.to_rgb(*p).map(|x| x.max(0.0)));
    part.measured.is_none().then_some(measured)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The grade as it was before its settings were worked out once per render (`GradePrep`), to
    // check the prepared one gives the same numbers.
    #[allow(clippy::all)]
    fn grade_reference(c: [f32; 3], g: &GradeSettings, t: &ToolTuning, ok: &Oklab, dirs: &[[f32; 2]; 4], spaces: [&Mat3; 4]) -> [f32; 3] {
        // The RGB the tints are added to: sRGB, or ProPhoto with `grade_rgb_pp`.
        let (to_srgb, from_srgb) = if t.grade_rgb_pp > 0.5 { (spaces[2], spaces[3]) } else { (spaces[0], spaces[1]) };
        let mut lab = ok.to_lab(c.map(|x| x.max(0.0)));
        let chroma0 = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
        // Lightness 0..1, bent by Balance (+ = more of the image counts as highlights).
        let v = lab[0].clamp(0.0, 1.0).powf(2f32.powf(-g.balance / 100.0 * t.grade_balance));
        // Blending sharpens (< 50) or softens (> 50) each range's reach.
        let e = t.grade_blend.max(0.1).powf((50.0 - g.blending) / 50.0);
        // Pure black takes no tint.
        let black = ramp(lab[0], 0.0, t.grade_black);
        // Near white takes almost no tint either (Lightroom keeps whites white).
        let black = black * (1.0 - ramp(lab[0], t.grade_white, 1.0));
        let ranges = [g.shadows, g.mids, g.highs, g.global];
        // The tints' strength by the pixel's saturation and brightness (measured
        // on the synthetic chart), on gamma-encoded sRGB like the chart's patches.
        let tint_surface = {
            let e = apply3(spaces[0], c).map(|x| crate::pipeline::srgb_encode(x.clamp(0.0, 1.0)));
            let [_, s, val] = rgb_to_hsv(e);
            lum_surface(&t.grade_tint_surface, s, val)
        };
        // The RGB-additive alternative (`grade_rgb`): the same tints added to
        // gamma-encoded sRGB, so they fade near black and clip near white by
        // each pixel's own colour.
        let gg = t.grade_rgb_gamma;
        let enc = |x: f32| x.signum() * x.abs().powf(1.0 / gg);
        let dec = |x: f32| x.signum() * x.abs().powf(gg);
        let rgb_mode = t.grade_rgb > 0.0;
        let mut e_rgb = if rgb_mode { apply3(to_srgb, c).map(enc) } else { [0.0; 3] };
        let ref_e = if rgb_mode { apply3(to_srgb, ok.to_rgb([0.6, 0.0, 0.0])).map(enc) } else { [0.0; 3] };
        let l0 = lab[0];
        // Each channel's own level, bent by Balance, for `grade_tint_chan`.
        let bend = 2f32.powf(-g.balance / 100.0 * t.grade_balance);
        let chan_v = e_rgb.map(|x| x.clamp(0.0, 1.0).powf(bend));
        // Each range's share of the tint here, for its lightness gains.
        let (mut k_sum, mut up, mut down) = (0.0f32, 0.0f32, 0.0f32);
        for (ri, ((r, curve), dir)) in ranges.iter().zip(&t.grade_curves).zip(dirs).enumerate() {
            let peak = curve.iter().cloned().fold(1e-6, f32::max);
            let reach = |level: f32| peak * (range_strength(curve, level) / peak).powf(e) * black;
            let w = reach(v);
            if r.sat != 0.0 {
                // The measured corrections for this range's settings.
                let turn = set_hue_at(&t.grade_set_turn[ri], r.hue);
                let (ts, tc) = turn.to_radians().sin_cos();
                let dir = &[dir[0] * tc - dir[1] * ts, dir[0] * ts + dir[1] * tc];
                let table_gain = set_gain_at(&t.grade_set_gain[ri], r.hue, r.sat).max(0.0);
                let light = set_hue_at(&t.grade_set_light[ri], r.hue);
                // Already-strong colours take less tint (`grade_chroma_damp`).
                let hr = r.hue.to_radians();
                let [g1, g2, g3, g4] = t.grade_hue_gain;
                // Lightroom's tints differ in strength by hue (green and magenta
                // strongest, yellow and blue weakest): two harmonics.
                let by_hue = (1.0 + g1 * hr.cos() + g2 * hr.sin() + g3 * (2.0 * hr).cos() + g4 * (2.0 * hr).sin()).max(0.0);
                let k_unit = r.sat / 100.0 * t.grade_sat * by_hue * table_gain * tint_surface / (1.0 + chroma0 * t.grade_chroma_damp);
                let k = k_unit * w;
                k_sum += k.abs();
                // Lightroom's lightness shift grows faster than the tint itself
                // (about with Saturation squared: ~0 at 25, a quarter at 50).
                let sat_shape = (r.sat.abs() / 100.0).powf(t.grade_lift_satpow);
                up += k.abs() * t.grade_lift_up[ri] * light * sat_shape;
                down += k.abs() * t.grade_lift_down[ri] * light * sat_shape;
                lab[1] += dir[0] * k;
                lab[2] += dir[1] * k;
                // Lightroom's tint also lifts lightness a little, by hue.
                lab[0] += k * (t.grade_lift[0] + t.grade_lift[1] * dir[0] + t.grade_lift[2] * dir[1]);
                if rgb_mode {
                    // The tint's direction in encoded RGB, taken at a mid grey...
                    let pushed = apply3(to_srgb, ok.to_rgb([0.6, dir[0] * 0.05, dir[1] * 0.05])).map(enc);
                    // ...or (`grade_pure`) the pure colour of the hue itself, less its
                    // grey: yellow then brightens, blue darkens, and green and magenta
                    // come out strongest, as measured in Lightroom.
                    let pure = hsv_to_rgb([r.hue + t.grade_pure_offset + turn, 1.0, 1.0]);
                    let mean = (pure[0] + pure[1] + pure[2]) / 3.0;
                    for i in 0..3 {
                        let along = (pushed[i] - ref_e[i]) / 0.05;
                        let d = along + ((pure[i] - mean) * t.grade_pure_gain - along) * t.grade_pure;
                        // The channel's range weight: the pixel's, or its own level's.
                        let wi = if t.grade_tint_chan > 0.0 { w + (reach(chan_v[i]) - w) * t.grade_tint_chan } else { w };
                        e_rgb[i] += d * k_unit * wi * t.grade_rgb_gain;
                    }
                }
            }
        }
        if rgb_mode {
            let rgb_tinted = ok.to_lab(apply3(from_srgb, e_rgb.map(|x| dec(x.clamp(0.0, 1.0)))).map(|x| x.max(0.0)));
            lab = std::array::from_fn(|i| lab[i] + (rgb_tinted[i] - lab[i]) * t.grade_rgb);
        }
        // The tints' lightness change, by range and by direction.
        if k_sum > 0.0 {
            let dl = lab[0] - l0;
            let gain = (if dl > 0.0 { up } else { down }) / k_sum;
            lab[0] = l0 + dl * gain;
        }
        // Luminance, like an exposure change on that range (colour strength
        // following it by a measured amount per range): a gain on light scales
        // OKLab by its cube root. Its own measured reach per range.
        let before_lum = lab;
        let (mut stops, mut colour_stops) = (0.0f32, 0.0f32);
        for ((r, curve), follow) in ranges.iter().zip(&t.grade_lum_curves).zip(t.grade_lum_chroma) {
            if r.lum != 0.0 {
                let s = r.lum / 100.0 * t.grade_lum * range_strength(curve, v) * black;
                stops += s;
                // How far colour strength follows: lifting shadows pales them in
                // Lightroom, lifting highlights enriches them.
                colour_stops += s * follow;
            }
        }
        if stops != 0.0 {
            let (k, kc) = (2f32.powf(stops / 3.0), 2f32.powf(colour_stops / 3.0));
            lab = [lab[0] * k, lab[1] * kc, lab[2] * kc];
            if t.grade_lum_rgb > 0.0 {
                // The alternative (`grade_lum_rgb`): the same gain per ProPhoto
                // channel, each by its own level, like an RGB tone curve - colours
                // then shift as channels move apart (most near white).
                let (to_pp, from_pp) = (spaces[2], spaces[3]);
                let bend = 2f32.powf(-g.balance / 100.0 * t.grade_balance);
                let lin = apply3(to_pp, ok.to_rgb(before_lum).map(|x| x.max(0.0)));
                let moved: [f32; 3] = std::array::from_fn(|i| {
                    let level = lin[i].clamp(0.0, 1.0).powf(1.0 / t.grade_lum_rgb_gamma.max(0.5)).powf(bend);
                    let s: f32 = ranges
                        .iter()
                        .zip(&t.grade_lum_curves)
                        .filter(|(r, _)| r.lum != 0.0)
                        .map(|(r, curve)| r.lum / 100.0 * t.grade_lum * range_strength(curve, level) * black)
                        .sum();
                    lin[i] * 2f32.powf(s)
                });
                let per_channel = ok.to_lab(apply3(from_pp, moved).map(|x| x.max(0.0)));
                lab = std::array::from_fn(|i| lab[i] + (per_channel[i] - lab[i]) * t.grade_lum_rgb);
            }
        }
        ok.to_rgb(lab).map(|x| x.max(0.0))
    }

    #[test]
    fn prepared_grade_matches_the_original() {
        let ok = Oklab::new();
        let mut seed = 7u64;
        let mut rnd = |lo: f32, hi: f32| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            lo + (hi - lo) * ((seed >> 40) as f32 / (1u64 << 24) as f32)
        };
        for profile in [crate::pipeline::Profile::ShinobiColour, crate::pipeline::Profile::AdobeColor] {
            let t = profile.tuning().tools;
            for _ in 0..40 {
                let range = |r: &mut dyn FnMut(f32, f32) -> f32| GradeRange { hue: r(0.0, 360.0), sat: r(-20.0, 100.0).max(0.0), lum: r(-100.0, 100.0) };
                let g = GradeSettings { shadows: range(&mut rnd), mids: range(&mut rnd), highs: range(&mut rnd), global: range(&mut rnd), blending: rnd(0.0, 100.0), balance: rnd(-100.0, 100.0) };
                let mut s = ToolSettings::default();
                s.grade = g;
                let prep = Prepared::new(&s, &t);
                let spaces = [&prep.to_srgb, &prep.from_srgb, &prep.to_prophoto, &prep.from_prophoto];
                let ranges = [g.shadows, g.mids, g.highs, g.global];
                let dirs: [[f32; 2]; 4] = std::array::from_fn(|i| {
                    let h = ranges[i].hue.to_radians();
                    let warp = t.grade_hue_warp[0] * h.cos() + t.grade_hue_warp[1] * h.sin();
                    hue_direction(ranges[i].hue + t.grade_hue_offset + t.grade_range_hue[i] + warp, t.grade_hue_space, &ok)
                });
                for _ in 0..300 {
                    let c = [rnd(0.0, 1.2), rnd(0.0, 1.2), rnd(0.0, 1.2)];
                    let new = grade_prepared(c, prep.grade_full.as_ref().unwrap(), &t, &ok, spaces);
                    let old = grade_reference(c, &g, &t, &ok, &dirs, spaces);
                    assert_eq!(new.map(f32::to_bits), old.map(f32::to_bits), "{c:?} {g:?}");
                    let f = fitted_grade(&g);
                    if let Some(fp) = &prep.grade_fitted {
                        assert_eq!(grade_prepared(c, fp, &t, &ok, spaces).map(f32::to_bits), grade_reference(c, &f, &t, &ok, &dirs, spaces).map(f32::to_bits));
                    }
                }
            }
        }
    }

    #[test]
    fn neutral_settings_change_nothing() {
        let s = ToolSettings::default();
        let t = ToolTuning::default();
        assert!(calibration_matrix(&s, &t, None).is_none());
        assert!(Prepared::new(&s, &t).is_identity());
    }

    #[test]
    fn calibration_keeps_white_and_moves_red_towards_yellow() {
        let s = ToolSettings { cal_red_hue: 100.0, ..Default::default() };
        let m = calibration_matrix(&s, &ToolTuning::default(), None).unwrap();
        let w = apply3(&m, [1.0, 1.0, 1.0]);
        // Within rounding of the round trip through ProPhoto.
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-3), "{w:?}");
        let red = apply3(&m, [1.0, 0.1, 0.1]);
        assert!(red[1] > red[2], "{red:?}");
    }

    #[test]
    fn reads_the_frontends_settings() {
        // The shape api.ts sends (engineSettingsFrom), trimmed.
        let json = r#"{
            "exposure": 0.5, "contrast": 0, "highlights": 0, "shadows": 0, "whites": 0, "blacks": 0,
            "vibrance": 0, "saturation": 0, "profile": "shinobiColour",
            "calShadowTint": 10, "calRedHue": -20, "calRedSat": 0, "calGreenHue": 0, "calGreenSat": 0,
            "calBlueHue": 0, "calBlueSat": 0, "vignette": -30,
            "mixer": [[0,0,0],[5,0,0],[0,0,0],[0,40,0],[0,0,0],[0,0,0],[0,0,0],[0,0,-10]],
            "curveLuma": [[0,0],[128,150],[255,255]], "curveRgb": [[0,0],[255,255]],
            "curveRed": [[0,0],[255,255]], "curveGreen": [[0,0],[255,255]], "curveBlue": [[0,0],[255,255]],
            "grade": {"shadows": {"hue": 210, "sat": 30, "lum": 0}, "mids": {"hue": 0, "sat": 0, "lum": 0},
                      "highs": {"hue": 30, "sat": 20, "lum": 5}, "global": {"hue": 0, "sat": 0, "lum": 0},
                      "blending": 60, "balance": -10}
        }"#;
        let s: crate::pipeline::RenderSettings = serde_json::from_str(json).expect("deserialise");
        assert_eq!(s.exposure, 0.5);
        assert_eq!(s.tools.cal_red_hue, -20.0);
        assert_eq!(s.tools.vignette, -30.0);
        assert_eq!(s.tools.mixer[3][1], 40.0);
        assert_eq!(s.tools.curve_luma.len(), 3);
        assert_eq!(s.tools.grade.shadows.hue, 210.0);
        assert_eq!(s.tools.grade.blending, 60.0);
        assert!(!Prepared::new(&s.tools, &ToolTuning::default()).is_identity());
    }

    #[test]
    fn band_weights_sum_to_one() {
        for h in (0..360).step_by(7) {
            let w = band_weights(h as f32, 1.0, &[0.0; 8]);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-4, "{h}: {w:?}");
        }
    }
}
