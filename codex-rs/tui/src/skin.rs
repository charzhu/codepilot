use std::sync::OnceLock;
use std::sync::RwLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::widgets::Clear;
use ratatui::widgets::Widget;

pub(crate) const DEFAULT_SKIN_ID: &str = "default";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Skin {
    pub(crate) id: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) palette: SkinPalette,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SkinPalette {
    pub(crate) background: Color,
    pub(crate) surface: Color,
    pub(crate) text: Color,
    pub(crate) muted: Color,
    pub(crate) primary: Color,
    pub(crate) secondary: Color,
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) danger: Color,
    pub(crate) selection: Color,
    pub(crate) border: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SkinEntry {
    pub(crate) id: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) is_default: bool,
}

const DEFAULT_SKIN_ENTRY: SkinEntry = SkinEntry {
    id: DEFAULT_SKIN_ID,
    display_name: "Default",
    description: "Use Codex's built-in TUI appearance",
    is_default: true,
};

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

pub(crate) const BUILTIN_SKINS: &[Skin] = &[
    Skin {
        id: "obsidian-bloom",
        display_name: "Obsidian Bloom",
        description: "Near-black glass with orchid, mint, and rose accents",
        palette: SkinPalette {
            background: rgb(7, 7, 10),
            surface: rgb(25, 22, 31),
            text: rgb(238, 235, 245),
            muted: rgb(149, 139, 166),
            primary: rgb(211, 132, 255),
            secondary: rgb(114, 226, 203),
            success: rgb(128, 232, 156),
            warning: rgb(244, 189, 97),
            danger: rgb(255, 111, 145),
            selection: rgb(58, 38, 78),
            border: rgb(127, 85, 158),
        },
    },
    Skin {
        id: "porcelain-ink",
        display_name: "Porcelain Ink",
        description: "Porcelain white, graphite text, cobalt, ruby, and jade",
        palette: SkinPalette {
            background: rgb(248, 245, 239),
            surface: rgb(239, 234, 224),
            text: rgb(30, 35, 43),
            muted: rgb(99, 104, 112),
            primary: rgb(26, 83, 166),
            secondary: rgb(176, 54, 76),
            success: rgb(32, 128, 88),
            warning: rgb(179, 113, 23),
            danger: rgb(176, 43, 54),
            selection: rgb(214, 226, 247),
            border: rgb(169, 177, 188),
        },
    },
    Skin {
        id: "deep-ocean",
        display_name: "Deep Ocean",
        description: "Abyss navy with aqua, seafoam, coral, and pearl",
        palette: SkinPalette {
            background: rgb(6, 25, 35),
            surface: rgb(12, 45, 58),
            text: rgb(223, 241, 243),
            muted: rgb(126, 162, 170),
            primary: rgb(79, 214, 220),
            secondary: rgb(125, 232, 180),
            success: rgb(106, 226, 152),
            warning: rgb(248, 173, 97),
            danger: rgb(255, 115, 109),
            selection: rgb(25, 83, 101),
            border: rgb(51, 129, 148),
        },
    },
    Skin {
        id: "paper-lantern",
        display_name: "Paper Lantern",
        description: "Warm paper, ink, vermilion, brass, and moss",
        palette: SkinPalette {
            background: rgb(255, 243, 214),
            surface: rgb(247, 225, 181),
            text: rgb(52, 39, 28),
            muted: rgb(123, 92, 61),
            primary: rgb(190, 65, 43),
            secondary: rgb(123, 96, 37),
            success: rgb(72, 125, 73),
            warning: rgb(185, 111, 30),
            danger: rgb(172, 44, 47),
            selection: rgb(236, 197, 132),
            border: rgb(189, 145, 78),
        },
    },
    Skin {
        id: "neon-circuit",
        display_name: "Neon Circuit",
        description: "Black-blue shell with cyan, hot pink, acid green, and amber",
        palette: SkinPalette {
            background: rgb(4, 8, 20),
            surface: rgb(13, 19, 42),
            text: rgb(226, 245, 255),
            muted: rgb(112, 132, 166),
            primary: rgb(0, 225, 255),
            secondary: rgb(255, 67, 176),
            success: rgb(164, 255, 60),
            warning: rgb(255, 196, 63),
            danger: rgb(255, 72, 90),
            selection: rgb(36, 42, 91),
            border: rgb(0, 154, 194),
        },
    },
    Skin {
        id: "evergreen-desk",
        display_name: "Evergreen Desk",
        description: "Forest green, ivory, sage, brass, and sky blue",
        palette: SkinPalette {
            background: rgb(16, 32, 25),
            surface: rgb(28, 49, 38),
            text: rgb(237, 235, 214),
            muted: rgb(151, 165, 134),
            primary: rgb(198, 168, 89),
            secondary: rgb(111, 174, 205),
            success: rgb(119, 185, 112),
            warning: rgb(230, 178, 83),
            danger: rgb(218, 96, 84),
            selection: rgb(48, 78, 59),
            border: rgb(88, 124, 93),
        },
    },
    Skin {
        id: "graphite-rose",
        display_name: "Graphite Rose",
        description: "Graphite, dusty rose, steel blue, mint, and pearl",
        palette: SkinPalette {
            background: rgb(22, 24, 28),
            surface: rgb(35, 37, 43),
            text: rgb(232, 229, 229),
            muted: rgb(151, 146, 151),
            primary: rgb(214, 134, 157),
            secondary: rgb(114, 154, 189),
            success: rgb(122, 204, 172),
            warning: rgb(224, 180, 101),
            danger: rgb(222, 100, 119),
            selection: rgb(58, 49, 58),
            border: rgb(111, 97, 108),
        },
    },
    Skin {
        id: "solar-flare",
        display_name: "Solar Flare",
        description: "Charcoal, ember orange, gold, cyan, and ash gray",
        palette: SkinPalette {
            background: rgb(17, 16, 15),
            surface: rgb(38, 30, 24),
            text: rgb(244, 232, 211),
            muted: rgb(162, 139, 115),
            primary: rgb(255, 126, 48),
            secondary: rgb(70, 198, 216),
            success: rgb(124, 208, 111),
            warning: rgb(255, 202, 82),
            danger: rgb(243, 78, 65),
            selection: rgb(79, 48, 25),
            border: rgb(166, 92, 48),
        },
    },
    Skin {
        id: "arctic-glass",
        display_name: "Arctic Glass",
        description: "Frosted blue, navy text, glacier cyan, and berry alerts",
        palette: SkinPalette {
            background: rgb(238, 247, 250),
            surface: rgb(220, 237, 243),
            text: rgb(24, 43, 66),
            muted: rgb(90, 116, 139),
            primary: rgb(22, 125, 173),
            secondary: rgb(135, 78, 162),
            success: rgb(42, 139, 110),
            warning: rgb(186, 128, 38),
            danger: rgb(190, 58, 91),
            selection: rgb(196, 225, 237),
            border: rgb(139, 178, 194),
        },
    },
    Skin {
        id: "desert-night",
        display_name: "Desert Night",
        description: "Deep indigo, sand, cactus green, clay, and turquoise",
        palette: SkinPalette {
            background: rgb(24, 21, 45),
            surface: rgb(45, 38, 68),
            text: rgb(239, 222, 189),
            muted: rgb(170, 146, 115),
            primary: rgb(76, 205, 196),
            secondary: rgb(220, 129, 87),
            success: rgb(116, 173, 99),
            warning: rgb(232, 173, 84),
            danger: rgb(219, 91, 91),
            selection: rgb(69, 57, 94),
            border: rgb(124, 99, 130),
        },
    },
    Skin {
        id: "phosphor-crt",
        display_name: "Phosphor CRT",
        description: "Black, phosphor green, amber, cyan, and dim gray",
        palette: SkinPalette {
            background: rgb(0, 8, 5),
            surface: rgb(6, 23, 15),
            text: rgb(199, 255, 208),
            muted: rgb(83, 138, 96),
            primary: rgb(79, 255, 116),
            secondary: rgb(73, 222, 218),
            success: rgb(85, 255, 131),
            warning: rgb(244, 190, 73),
            danger: rgb(255, 86, 86),
            selection: rgb(13, 54, 31),
            border: rgb(55, 128, 76),
        },
    },
    Skin {
        id: "plum-terminal",
        display_name: "Plum Terminal",
        description: "Aubergine, mauve, peach, lime, and powder blue",
        palette: SkinPalette {
            background: rgb(33, 20, 42),
            surface: rgb(54, 34, 64),
            text: rgb(242, 227, 243),
            muted: rgb(171, 134, 177),
            primary: rgb(245, 164, 133),
            secondary: rgb(153, 196, 255),
            success: rgb(172, 225, 108),
            warning: rgb(237, 191, 94),
            danger: rgb(232, 102, 127),
            selection: rgb(79, 49, 91),
            border: rgb(141, 97, 151),
        },
    },
    Skin {
        id: "blueprint",
        display_name: "Blueprint",
        description: "Deep blueprint blue with pale borders, orange, and green",
        palette: SkinPalette {
            background: rgb(8, 31, 72),
            surface: rgb(15, 52, 106),
            text: rgb(234, 244, 255),
            muted: rgb(150, 181, 217),
            primary: rgb(133, 202, 255),
            secondary: rgb(255, 166, 83),
            success: rgb(122, 220, 151),
            warning: rgb(255, 201, 94),
            danger: rgb(255, 111, 111),
            selection: rgb(24, 75, 143),
            border: rgb(97, 151, 205),
        },
    },
    Skin {
        id: "candy-shell",
        display_name: "Candy Shell",
        description: "Off-white shell with cherry, teal, lemon, and lavender",
        palette: SkinPalette {
            background: rgb(255, 250, 246),
            surface: rgb(247, 238, 247),
            text: rgb(42, 42, 54),
            muted: rgb(111, 104, 122),
            primary: rgb(214, 45, 88),
            secondary: rgb(0, 153, 153),
            success: rgb(54, 154, 104),
            warning: rgb(204, 151, 30),
            danger: rgb(190, 39, 69),
            selection: rgb(230, 216, 248),
            border: rgb(196, 170, 215),
        },
    },
    Skin {
        id: "monochrome-pro",
        display_name: "Monochrome Pro",
        description: "Strict grayscale with one cool blue accent",
        palette: SkinPalette {
            background: rgb(12, 12, 13),
            surface: rgb(30, 30, 32),
            text: rgb(238, 238, 238),
            muted: rgb(150, 150, 154),
            primary: rgb(118, 171, 235),
            secondary: rgb(197, 197, 203),
            success: rgb(176, 214, 176),
            warning: rgb(224, 202, 151),
            danger: rgb(226, 142, 142),
            selection: rgb(54, 54, 58),
            border: rgb(104, 104, 110),
        },
    },
];

static ACTIVE_SKIN: OnceLock<RwLock<Option<&'static Skin>>> = OnceLock::new();

fn active_skin_lock() -> &'static RwLock<Option<&'static Skin>> {
    ACTIVE_SKIN.get_or_init(|| RwLock::new(None))
}

pub(crate) fn list_skins() -> Vec<SkinEntry> {
    std::iter::once(DEFAULT_SKIN_ENTRY)
        .chain(BUILTIN_SKINS.iter().map(|skin| SkinEntry {
            id: skin.id,
            display_name: skin.display_name,
            description: skin.description,
            is_default: false,
        }))
        .collect()
}

pub(crate) fn skin_by_id(id: &str) -> Option<&'static Skin> {
    BUILTIN_SKINS.iter().find(|skin| skin.id == id)
}

pub(crate) fn is_valid_skin_id(id: &str) -> bool {
    id == DEFAULT_SKIN_ID || skin_by_id(id).is_some()
}

pub(crate) fn normalize_skin_id(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

pub(crate) fn configured_skin_value(id: &str) -> Option<String> {
    (id != DEFAULT_SKIN_ID).then(|| id.to_string())
}

pub(crate) fn current_skin() -> Option<&'static Skin> {
    active_skin_lock().read().ok().and_then(|guard| *guard)
}

pub(crate) fn current_skin_id() -> Option<&'static str> {
    current_skin().map(|skin| skin.id)
}

pub(crate) fn base_style() -> Option<Style> {
    current_skin().map(base_style_for)
}

pub(crate) fn set_runtime_skin_by_id(id: &str) -> bool {
    let skin = if id == DEFAULT_SKIN_ID {
        None
    } else if let Some(skin) = skin_by_id(id) {
        Some(skin)
    } else {
        return false;
    };
    set_runtime_skin(skin);
    true
}

pub(crate) fn set_runtime_skin_by_config_value(id: Option<&str>) {
    let skin = id.and_then(skin_by_id);
    set_runtime_skin(skin);
}

pub(crate) fn set_skin_override(id: Option<String>) -> Option<String> {
    let Some(id) = id else {
        set_runtime_skin(None);
        return None;
    };
    let id = normalize_skin_id(&id);
    if id == DEFAULT_SKIN_ID {
        set_runtime_skin(None);
        return None;
    }
    if let Some(skin) = skin_by_id(&id) {
        set_runtime_skin(Some(skin));
        None
    } else {
        set_runtime_skin(None);
        Some(format!(
            "Skin \"{id}\" not found. Using the default TUI appearance. Use /skin to choose a built-in skin."
        ))
    }
}

fn set_runtime_skin(skin: Option<&'static Skin>) {
    if let Ok(mut guard) = active_skin_lock().write() {
        *guard = skin;
    }
}

pub(crate) fn clear_area(area: Rect, buf: &mut Buffer) {
    if let Some(skin) = current_skin() {
        fill_area(area, buf, base_style_for(skin));
    } else {
        Clear.render(area, buf);
    }
}

pub(crate) fn paint_background(area: Rect, buf: &mut Buffer) {
    if let Some(skin) = current_skin() {
        fill_area(area, buf, base_style_for(skin));
    }
}

pub(crate) fn user_message_style() -> Option<Style> {
    current_skin().map(|skin| surface_style_for(skin))
}

pub(crate) fn accent_style() -> Option<Style> {
    current_skin().map(|skin| {
        Style::default()
            .fg(skin.palette.primary)
            .bg(skin.palette.background)
            .add_modifier(Modifier::BOLD)
    })
}

pub(crate) fn surface_style() -> Option<Style> {
    current_skin().map(surface_style_for)
}

fn base_style_for(skin: &Skin) -> Style {
    Style::default()
        .fg(skin.palette.text)
        .bg(skin.palette.background)
}

fn surface_style_for(skin: &Skin) -> Style {
    Style::default()
        .fg(skin.palette.text)
        .bg(skin.palette.surface)
}

fn fill_area(area: Rect, buf: &mut Buffer, style: Style) {
    let buf_area = buf.area();
    let min_x = area.x.max(buf_area.x);
    let min_y = area.y.max(buf_area.y);
    let max_x = area
        .x
        .saturating_add(area.width)
        .min(buf_area.x.saturating_add(buf_area.width));
    let max_y = area
        .y
        .saturating_add(area.height)
        .min(buf_area.y.saturating_add(buf_area.height));
    for y in min_y..max_y {
        for x in min_x..max_x {
            buf[(x, y)].set_symbol(" ").set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn default_skin_id_is_valid_but_not_persisted() {
        assert!(is_valid_skin_id(DEFAULT_SKIN_ID));
        assert_eq!(configured_skin_value(DEFAULT_SKIN_ID), None);
    }

    #[test]
    fn built_in_skins_are_unique_and_listed_after_default() {
        let entries = list_skins();

        assert_eq!(entries[0], DEFAULT_SKIN_ENTRY);
        for skin in BUILTIN_SKINS {
            assert_eq!(skin_by_id(skin.id), Some(skin));
            assert_eq!(
                entries.iter().filter(|entry| entry.id == skin.id).count(),
                1
            );
        }
    }
}
