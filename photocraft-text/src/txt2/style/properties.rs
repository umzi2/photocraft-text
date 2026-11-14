/// Character properties Photoshop stores inside a `Txt2` style run, with the
/// key it uses in the run's `/6` dictionary.
///
/// Every slot listed here is either measured by diffing two documents that
/// differ in exactly one parameter (`out/probe2/*.psd`) or by authoring the
/// slot and letting Photoshop re-lay-out and re-save the layer
/// (`out/verify/*.psd`). Slots with no evidence are deliberately absent:
/// manual kerning amount `/10` (Photoshop reads it as 16.16 fixed point, see
/// `out/verify/mk100_res.psd`) and the faux-style stroke width keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunProperty {
    /// `/6 /0`: index into the block's font table (`ensure_font`).
    FontIndex,
    /// `/6 /1`, points (`out/mr`, `plain_latin`).
    FontSize,
    /// `/6 /2`, boolean: faux bold (`out/probe2/fauxbold.psd`, `false -> true`).
    FauxBold,
    /// `/6 /3`, boolean: faux italic (`out/probe2/fauxitalic.psd`).
    FauxItalic,
    /// `/6 /4`, boolean: derive the line advance from the auto-leading fraction
    /// (`out/probe2/lead40.psd`, `true -> false`).
    AutoLeading,
    /// `/6 /5`, points (auto leading stores `size * 1.2`;
    /// `out/probe2/lead40.psd`, `57.6 -> 40.0`).
    Leading,
    /// `/6 /6`, fraction (1.0 = 100%; `out/probe2/hscale150.psd`).
    HorizontalScale,
    /// `/6 /7`, fraction (1.0 = 100%).
    VerticalScale,
    /// `/6 /8`, 1/1000 em (`out/probe2/tracking100.psd`, `0 -> 100`).
    Tracking,
    /// `/6 /9`, points (positive raises).
    BaselineShift,
    /// `/6 /11`: `0` manual, `1` metrics, `2` optical
    /// (`out/probe2/kerning_manual.psd` `1 -> 0`, `kerning_optical.psd` `1 -> 2`).
    KerningType,
    /// `/6 /13`: `0` normal, `1` superscript, `2` subscript. Photoshop keeps
    /// `FontBaseline 0` in `EngineData` for all three (measured with the
    /// writer-parity oracle: a run authored as super/subscript keeps
    /// `Txt2 /6/13` and re-saves `FontBaseline 0`).
    FontBaseline,
    /// `/6 /12`: `0` normal, `1` small caps, `2` all caps
    /// (`out/verify/caps.psd` authored `2`; Photoshop re-rendered ALL CAPS in
    /// its PDF and kept `2` after re-save).
    Capitalization,
    /// `/6 /15`: `0` off, `2` strike-box (`out/verify/strike.psd` authored `2`,
    /// kept after re-save).
    Strikethrough,
    /// `/6 /16`: `0` off, `1` right, `2` left (`out/verify/underline.psd`,
    /// both sides kept after re-save).
    Underline,
    /// `/6 /18`, boolean: standard ligatures. `out/verify/ligoff.psd` authored
    /// `false`; Photoshop re-saved `Ligatures false`.
    Ligatures,
    /// `/6 /19`, boolean: alternate (discretionary) ligatures.
    AlternateLigatures,
    /// `/6 /30`: old-style figures; Photoshop's own value for "on" in the
    /// measured files is `2` (`out/verify/oldstyle.psd`, kept after re-save).
    OldStyle,
    /// `/6 /38`: language code (`out/verify/lang.psd` authored `14`; Photoshop
    /// re-saved `Language 14`).
    Language,
    /// `/6 /52`, boolean: no break (keep the run on one line).
    NoBreak,
    /// `/6 /20`, boolean: contextual alternates (`calt`; Photoshop's default is
    /// off, so it also writes an explicit `false`).
    ContextualLigatures,
    /// `/6 /23`, boolean: fractions (`frac`).
    Fractions,
    /// `/6 /24`, boolean: ordinals (`ordn`).
    Ordinals,
    /// `/6 /25`, boolean: swash (`swsh`).
    Swash,
    /// `/6 /26`, boolean: titling (`titl`).
    Titling,
    /// `/6 /28`, boolean: stylistic alternates (`salt`).
    StylisticAlternates,
    /// `/6 /29`, boolean: ornaments (`ornm`).
    Ornaments,
    /// `/6 /85`: Stylistic Sets bit mask, bit n selects `ss{n+1}` (n = 0…19).
    StylisticSets,
    /// `/6 /53`: fill paint (`SimplePaint`, see [`set_run_fill_color`](super::set_run_fill_color)).
    FillPaint,
    /// `/6 /54`: stroke paint (`SimplePaint`).
    StrokePaint,
}

impl RunProperty {
    /// The character-sheet key Photoshop stores this property under.
    pub fn key(self) -> &'static str {
        match self {
            RunProperty::FontIndex => "0",
            RunProperty::FontSize => "1",
            RunProperty::FauxBold => "2",
            RunProperty::FauxItalic => "3",
            RunProperty::AutoLeading => "4",
            RunProperty::Leading => "5",
            RunProperty::HorizontalScale => "6",
            RunProperty::VerticalScale => "7",
            RunProperty::Tracking => "8",
            RunProperty::BaselineShift => "9",
            RunProperty::ContextualLigatures => "20",
            RunProperty::Fractions => "23",
            RunProperty::Ordinals => "24",
            RunProperty::Swash => "25",
            RunProperty::Titling => "26",
            RunProperty::StylisticAlternates => "28",
            RunProperty::Ornaments => "29",
            RunProperty::StylisticSets => "85",
            RunProperty::KerningType => "11",
            RunProperty::Capitalization => "12",
            RunProperty::FontBaseline => "13",
            RunProperty::Strikethrough => "15",
            RunProperty::Underline => "16",
            RunProperty::Ligatures => "18",
            RunProperty::AlternateLigatures => "19",
            RunProperty::OldStyle => "30",
            RunProperty::Language => "38",
            RunProperty::NoBreak => "52",
            RunProperty::FillPaint => "53",
            RunProperty::StrokePaint => "54",
        }
    }

    /// The property stored under `key`, or `None` for a slot this module does
    /// not know.
    pub fn from_key(key: &str) -> Option<Self> {
        [
            Self::FontIndex,
            Self::FontSize,
            Self::FauxBold,
            Self::FauxItalic,
            Self::AutoLeading,
            Self::Leading,
            Self::HorizontalScale,
            Self::VerticalScale,
            Self::Tracking,
            Self::BaselineShift,
            Self::KerningType,
            Self::Capitalization,
            Self::FontBaseline,
            Self::Strikethrough,
            Self::Underline,
            Self::Ligatures,
            Self::AlternateLigatures,
            Self::OldStyle,
            Self::Language,
            Self::NoBreak,
            Self::ContextualLigatures,
            Self::Fractions,
            Self::Ordinals,
            Self::Swash,
            Self::Titling,
            Self::StylisticAlternates,
            Self::Ornaments,
            Self::StylisticSets,
            Self::FillPaint,
            Self::StrokePaint,
        ]
        .into_iter()
        .find(|p| p.key() == key)
    }

    /// Whether the slot holds a paint dictionary rather than a scalar.
    pub fn is_paint(self) -> bool {
        matches!(self, Self::FillPaint | Self::StrokePaint)
    }
}

/// How Photoshop kerns a run (`Txt2` `/6/11`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KerningType {
    /// `0`: manual values, stored per pair or as the run's `Kerning` amount.
    Manual,
    /// `1`: pair kerning from the font's `kern`/`GPOS` tables.
    Metrics,
    /// `2`: optical, Adobe's area-based spacing (`optical.rs` approximates it).
    Optical,
}

impl KerningType {
    pub fn code(self) -> i64 {
        match self {
            Self::Manual => 0,
            Self::Metrics => 1,
            Self::Optical => 2,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Manual,
            1 => Self::Metrics,
            2 => Self::Optical,
            _ => return None,
        })
    }

    /// How `EngineData` represents this mode: it only carries the
    /// metrics/manual switch (`AutoKerning`), so optical stays on the metrics
    /// side — Photoshop's own `out/probe2/kerning_optical.psd` keeps
    /// `AutoKerning true`.
    pub fn engine_auto_kerning(self) -> bool {
        !matches!(self, Self::Manual)
    }
}

/// Character case mapping of a run (`Txt2` `/6/12`, `EngineData` `FontCaps`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capitalization {
    /// `0`.
    Normal,
    /// `1`: small caps.
    SmallCaps,
    /// `2`: all caps (`out/verify/caps_res.psd` renders ALL CAPS).
    AllCaps,
}

impl Capitalization {
    pub fn code(self) -> i64 {
        match self {
            Self::Normal => 0,
            Self::SmallCaps => 1,
            Self::AllCaps => 2,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Normal,
            1 => Self::SmallCaps,
            2 => Self::AllCaps,
            _ => return None,
        })
    }

    /// `EngineData` `FontCaps` uses the same three codes.
    pub fn engine_font_caps(self) -> i64 {
        self.code()
    }
}

/// Underline position of a run (`Txt2` `/6/16`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    /// `0`: no underline.
    Off,
    /// `1`: underline on the right of vertical text.
    Right,
    /// `2`: underline on the left (ordinary horizontal text).
    Left,
}

impl Underline {
    pub fn code(self) -> i64 {
        match self {
            Self::Off => 0,
            Self::Right => 1,
            Self::Left => 2,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Off,
            1 => Self::Right,
            2 => Self::Left,
            _ => return None,
        })
    }

    /// `EngineData` only has a boolean `Underline`.
    pub fn engine_underline(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Strikethrough style of a run (`Txt2` `/6/15`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strikethrough {
    /// `0`: none.
    Off,
    /// `2`: a strike box (`out/verify/strike_res.psd` keeps `2`).
    StrikeBox,
}

impl Strikethrough {
    pub fn code(self) -> i64 {
        match self {
            Self::Off => 0,
            Self::StrikeBox => 2,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Off,
            2 => Self::StrikeBox,
            _ => return None,
        })
    }

    /// `EngineData` only has a boolean `Strikethrough`.
    pub fn engine_strikethrough(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// The `/6/30` value Photoshop keeps for old-style figures switched on, as
/// measured in `out/verify/oldstyle.psd`. There is no `EngineData` key for this
/// flag, so it reaches `Txt2` only.
pub const OLD_STYLE_ON: i64 = 2;

/// Language codes measured in a Photoshop re-save. The block stores an integer
/// index into Photoshop's language table; unknown values are still legal, so
/// [`set_run_language_code`](super::set_run_language_code) accepts a raw code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    /// `0`: Photoshop's default.
    Default,
    /// `14`: English (UK), measured in `out/verify/lang_res.psd`.
    EnglishUk,
    /// `42`: German, measured in the round-2 probe.
    German,
}

impl Language {
    pub fn code(self) -> i64 {
        match self {
            Self::Default => 0,
            Self::EnglishUk => 14,
            Self::German => 42,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Default,
            14 => Self::EnglishUk,
            42 => Self::German,
            _ => return None,
        })
    }
}
