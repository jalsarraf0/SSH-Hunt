#![forbid(unsafe_code)]

use protocol::{MissionState, Mode};

// ── ANSI constants ──────────────────────────────────────────────────────────

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";

// ── Enums ───────────────────────────────────────────────────────────────────

/// Status tag for boot-sequence lines.
#[derive(Debug, Clone, Copy)]
pub enum BootStatus {
    Ok,
    Warn,
    Fail,
    Loading,
}

/// State for inline HUD status indicators.
#[derive(Debug, Clone, Copy)]
pub enum StatusState {
    Ok,
    Warn,
    Alert,
    Inactive,
}

// ── Theme ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub primary: &'static str,
    pub accent: &'static str,
    pub warn: &'static str,
    pub flash: &'static str,
    pub muted: &'static str,
    pub dim: &'static str,
    pub highlight: &'static str,
    pub cyan: &'static str,
    pub yellow: &'static str,
}

impl Theme {
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Training => Self {
                primary: "\x1b[38;5;46m",
                accent: "\x1b[38;5;120m",
                warn: "\x1b[38;5;154m",
                flash: "\x1b[48;5;22m\x1b[38;5;46m",
                muted: "\x1b[38;5;244m",
                dim: "\x1b[2m\x1b[38;5;46m",
                highlight: "\x1b[1m\x1b[38;5;46m",
                cyan: "\x1b[38;5;51m",
                yellow: "\x1b[38;5;226m",
            },
            Mode::NetCity => Self {
                primary: "\x1b[38;5;141m",
                accent: "\x1b[38;5;213m",
                warn: "\x1b[38;5;177m",
                flash: "\x1b[48;5;53m\x1b[38;5;213m",
                muted: "\x1b[38;5;245m",
                dim: "\x1b[2m\x1b[38;5;141m",
                highlight: "\x1b[1m\x1b[38;5;141m",
                cyan: "\x1b[38;5;51m",
                yellow: "\x1b[38;5;226m",
            },
            Mode::Redline => Self {
                primary: "\x1b[38;5;196m",
                accent: "\x1b[38;5;203m",
                warn: "\x1b[38;5;210m",
                flash: "\x1b[5m\x1b[38;5;15m\x1b[48;5;160m",
                muted: "\x1b[38;5;245m",
                dim: "\x1b[2m\x1b[38;5;196m",
                highlight: "\x1b[1m\x1b[38;5;196m",
                cyan: "\x1b[38;5;51m",
                yellow: "\x1b[38;5;226m",
            },
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn clipped(text: &str, width: usize) -> String {
    text.chars().take(width).collect::<String>()
}

/// Count visible (display) characters, ignoring ANSI escape sequences.
/// This is critical for alignment — ANSI codes like `\x1b[38;5;46m` take
/// zero columns on screen but many bytes/chars in the string.
pub fn visible_len(s: &str) -> usize {
    let mut count = 0;
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            count += 1;
        }
    }
    count
}

/// Pad `text` (which may contain ANSI codes) to exactly `target_visible_width`
/// visible characters by appending spaces. If already wider, returns as-is.
pub fn pad_visible(text: &str, target_visible_width: usize) -> String {
    let vis = visible_len(text);
    if vis >= target_visible_width {
        text.to_string()
    } else {
        format!("{}{}", text, " ".repeat(target_visible_width - vis))
    }
}

/// Center `text` (plain, no ANSI) within `width`.
fn centered(text: &str, width: usize) -> String {
    let text_len = text.chars().count();
    if text_len >= width {
        return clipped(text, width);
    }
    let pad = (width - text_len) / 2;
    format!("{}{}", " ".repeat(pad), text)
}

// ── Decorative / Glitch Art ─────────────────────────────────────────────────

/// Glitch-art horizontal divider: `▓▒░━━━━━━━━━━━━━━━━━━━━━━━━━━━░▒▓`
pub fn glitch_divider(mode: Mode, columns: usize, unicode: bool) -> String {
    let theme = Theme::for_mode(mode);
    if columns < 10 {
        return String::new();
    }
    if unicode {
        let fill_width = columns.saturating_sub(6);
        format!(
            "{cyan}▓▒░{dim}{}{RESET}{cyan}░▒▓{RESET}\n",
            "━".repeat(fill_width),
            cyan = theme.cyan,
            dim = theme.dim,
        )
    } else {
        let fill_width = columns.saturating_sub(6);
        format!(
            "{cyan}#={dim}{}{RESET}{cyan}=#{RESET}\n",
            "-".repeat(fill_width),
            cyan = theme.cyan,
            dim = theme.dim,
        )
    }
}

/// Subtle CRT scanline — very faint atmosphere.
pub fn scanline(mode: Mode, columns: usize, unicode: bool) -> String {
    let theme = Theme::for_mode(mode);
    if columns < 10 || !unicode {
        return format!("{}{}{RESET}\n", theme.dim, "-".repeat(columns.min(60)));
    }
    let width = columns.min(80);
    format!("{}{}{RESET}\n", theme.dim, "░".repeat(width))
}

/// Cyberpunk neon section header: `┃ ▸ TITLE ◂ ┃`
pub fn neon_header(mode: Mode, text: &str, columns: usize, unicode: bool) -> String {
    let theme = Theme::for_mode(mode);
    let text = clipped(text, columns.saturating_sub(12).max(4));
    if unicode {
        format!(
            "{cyan}┃ ▸ {hl}{text}{RESET} {cyan}◂ ┃{RESET}\n",
            cyan = theme.cyan,
            hl = theme.highlight,
        )
    } else {
        format!(
            "{cyan}| > {hl}{text}{RESET} {cyan}< |{RESET}\n",
            cyan = theme.cyan,
            hl = theme.highlight,
        )
    }
}

// ── Boot Sequence ───────────────────────────────────────────────────────────

/// Single boot-sequence line with fixed-width label, dot-leader, and
/// right-aligned status message. All lines align perfectly regardless of
/// label or message length.
///
/// ```text
///  [OK]  NEURAL LINK ... connected
///  [!!]  VAULT-SAT-9 ... degraded
///  [OK]  SYSTEM READY .. NOOB clearance granted
/// ```
pub fn boot_line(
    mode: Mode,
    label: &str,
    message: &str,
    status: BootStatus,
    columns: usize,
) -> String {
    let theme = Theme::for_mode(mode);

    let (tag, tag_color) = match status {
        BootStatus::Ok => ("OK", theme.accent),
        BootStatus::Warn => ("!!", theme.yellow),
        BootStatus::Fail => ("XX", theme.warn),
        BootStatus::Loading => ("~~", theme.muted),
    };

    // Compact mode for narrow terminals
    if columns < 40 {
        return format!(
            " {tc}[{tag}]{RESET} {label} {dim}{msg}{RESET}\n",
            tc = tag_color,
            dim = theme.dim,
            msg = message,
        );
    }

    // Fixed-width label column (16 chars) for perfect alignment.
    // " [OK]  " = 7 chars, then label padded to 16, then " ", then dots, then " ", then message
    let label_col = 16;
    let padded_label = format!("{:<width$}", label, width = label_col);

    // Total fixed prefix: " [OK]  LABEL_16_CHARS " = 7 + 16 + 1 = 24
    let prefix_used = 7 + label_col + 1;
    // Suffix: " message"
    let suffix_used = 1 + message.chars().count();
    let dot_count = columns
        .saturating_sub(prefix_used + suffix_used)
        .clamp(3, 50);

    format!(
        " {tc}[{tag}]{RESET}  {padded_label} {dim}{dots}{RESET} {msg}\n",
        tc = tag_color,
        dots = ".".repeat(dot_count),
        dim = theme.dim,
        msg = message,
    )
}

// ── Splash Logo ─────────────────────────────────────────────────────────────

/// Large ASCII art SSH-HUNT logo with three width tiers.
pub fn splash_logo(mode: Mode, columns: usize, unicode: bool) -> String {
    let theme = Theme::for_mode(mode.clone());
    let mut out = String::new();

    if columns >= 70 && unicode {
        // ── Full logo (70+ columns) ──
        out.push_str(&glitch_divider(mode.clone(), columns, unicode));
        out.push('\n');

        // All lines are exactly 68 visible characters wide (padded/trimmed).
        let logo_lines = [
            "███████╗███████╗██╗  ██╗      ██╗  ██╗██╗   ██╗███╗   ██╗████████╗",
            "██╔════╝██╔════╝██║  ██║      ██║  ██║██║   ██║████╗  ██║╚══██╔══╝",
            "███████╗███████╗███████║█████╗███████║██║   ██║██╔██╗ ██║   ██║   ",
            "╚════██║╚════██║██╔══██║╚════╝██╔══██║██║   ██║██║╚██╗██║   ██║   ",
            "███████║███████║██║  ██║      ██║  ██║╚██████╔╝██║ ╚████║   ██║   ",
            "╚══════╝╚══════╝╚═╝  ╚═╝      ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝   ╚═╝",
        ];

        for line in &logo_lines {
            // Pad each line to exactly 68 chars before centering.
            let padded = format!("{:<68}", line);
            out.push_str(&format!(
                "{primary}{}{RESET}\n",
                centered(&padded, columns),
                primary = theme.primary,
            ));
        }

        out.push('\n');
        let tagline = "// Jack in. Hack deep. Don't get caught. //";
        out.push_str(&format!(
            "{dim}{}{RESET}\n",
            centered(tagline, columns),
            dim = theme.dim,
        ));
        out.push('\n');
        out.push_str(&glitch_divider(mode, columns, unicode));
    } else if columns >= 40 {
        // ── Compact logo (40-69 columns) ──
        out.push_str(&glitch_divider(mode.clone(), columns, unicode));

        let logo_lines = [
            "╔═╗╔═╗╦ ╦   ╦ ╦╦ ╦╔╗╔╔╦╗",
            "╚═╗╚═╗╠═╣───╠═╣║ ║║║║ ║ ",
            "╚═╝╚═╝╩ ╩   ╩ ╩╚═╝╝╚╝ ╩ ",
        ];

        for line in &logo_lines {
            let padded = format!("{:<27}", line);
            out.push_str(&format!(
                "{primary}{}{RESET}\n",
                centered(&padded, columns),
                primary = theme.primary,
            ));
        }

        let tagline = "// Jack in. Hack deep. //";
        out.push_str(&format!(
            "{dim}{}{RESET}\n",
            centered(tagline, columns),
            dim = theme.dim,
        ));
        out.push_str(&glitch_divider(mode, columns, unicode));
    } else {
        // ── Narrow (< 40 columns) ──
        if unicode {
            out.push_str(&format!(
                "{cyan}░▒▓ {hl}SSH-HUNT{RESET} {cyan}▓▒░{RESET}\n",
                cyan = theme.cyan,
                hl = theme.highlight,
            ));
        } else {
            out.push_str(&format!("{hl}[ SSH-HUNT ]{RESET}\n", hl = theme.highlight,));
        }
    }

    out
}

// ── HUD Panels ──────────────────────────────────────────────────────────────

/// Bordered panel with neon-accented cutout title. Uses `visible_len` to
/// correctly pad body lines that contain ANSI escape codes, so the right
/// border always aligns perfectly.
pub fn titled_panel(
    mode: Mode,
    title: &str,
    body_lines: &[String],
    columns: usize,
    unicode: bool,
) -> String {
    let theme = Theme::for_mode(mode);

    if columns < 30 {
        let mut out = format!("{hl}[ {title} ]{RESET}\n", hl = theme.highlight);
        for line in body_lines {
            out.push_str(&format!("  {line}\n"));
        }
        return out;
    }

    // inner_width = visible chars between the left and right border characters.
    // Layout: "║ " + <inner_width visible chars> + " ║"  = inner_width + 4 total.
    let inner_width = columns.saturating_sub(4).clamp(20, 76);
    let title_display = clipped(title, inner_width.saturating_sub(6));
    let mut out = String::new();

    if unicode {
        // Top: ╔══╡ TITLE ╞══════════════╗
        let title_vis_len = title_display.chars().count();
        // "╡ " + title + " ╞" = title_vis_len + 4
        let border_used = title_vis_len + 4 + 2; // +2 for left_fill
        let right_fill = inner_width.saturating_sub(border_used) + 2; // +2 to match total
        out.push_str(&format!(
            "{cyan}╔══╡ {hl}{title_display}{RESET} {cyan}╞{}╗{RESET}\n",
            "═".repeat(right_fill),
            cyan = theme.cyan,
            hl = theme.highlight,
        ));

        for line in body_lines {
            let padded = pad_visible(line, inner_width);
            out.push_str(&format!(
                "{cyan}║{RESET} {padded} {cyan}║{RESET}\n",
                cyan = theme.cyan,
            ));
        }

        out.push_str(&format!(
            "{cyan}╚{}╝{RESET}\n",
            "═".repeat(inner_width + 2),
            cyan = theme.cyan,
        ));
    } else {
        // ASCII: +--[ TITLE ]--------+
        let title_vis_len = title_display.chars().count();
        let right_fill = inner_width.saturating_sub(title_vis_len + 5);
        out.push_str(&format!(
            "{cyan}+--[ {hl}{title_display}{RESET} {cyan}]{}+{RESET}\n",
            "-".repeat(right_fill),
            cyan = theme.cyan,
            hl = theme.highlight,
        ));

        for line in body_lines {
            let padded = pad_visible(line, inner_width);
            out.push_str(&format!(
                "{cyan}|{RESET} {padded} {cyan}|{RESET}\n",
                cyan = theme.cyan,
            ));
        }

        out.push_str(&format!(
            "{cyan}+{}+{RESET}\n",
            "-".repeat(inner_width + 2),
            cyan = theme.cyan,
        ));
    }

    out
}

/// Mid-panel horizontal divider for use inside a `titled_panel`.
/// Returns a plain string (no border chars) sized to `inner_width`.
pub fn panel_divider_line(columns: usize, unicode: bool) -> String {
    let inner_width = columns.saturating_sub(4).clamp(20, 76);
    if unicode {
        "─".repeat(inner_width)
    } else {
        "-".repeat(inner_width)
    }
}

/// Two-column key-value layout. Falls back to single column below 56 cols.
/// All values are plain-text only — no ANSI codes in `pairs` values.
/// Returns lines that are exactly `target_width` visible characters wide.
pub fn two_column_kv(mode: Mode, pairs: &[(&str, &str)], columns: usize) -> Vec<String> {
    let theme = Theme::for_mode(mode);
    let inner_width = columns.saturating_sub(4).clamp(20, 76);

    if columns < 56 || pairs.len() < 2 {
        return pairs
            .iter()
            .map(|(k, v)| {
                let line = format!("{accent}{k:<14}{RESET} {v}", accent = theme.accent);
                pad_visible(&line, inner_width)
            })
            .collect();
    }

    // Two-column: each column gets half the inner width.
    let col_width = inner_width / 2;
    let mut lines = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        let (lk, lv) = pairs[i];
        let left = format!("{accent}{lk:<12}{RESET} {lv}", accent = theme.accent);
        let left_padded = pad_visible(&left, col_width);

        if i + 1 < pairs.len() {
            let (rk, rv) = pairs[i + 1];
            let right = format!("{accent}{rk:<12}{RESET} {rv}", accent = theme.accent);
            let right_padded = pad_visible(&right, col_width);
            let combined = format!("{left_padded}{right_padded}");
            lines.push(pad_visible(&combined, inner_width));
            i += 2;
        } else {
            lines.push(pad_visible(&left_padded, inner_width));
            i += 1;
        }
    }
    lines
}

/// Inline status dot: `●` / `◆` / `✖` / `○` (or ASCII equivalents).
pub fn status_dot(mode: Mode, state: StatusState, unicode: bool) -> String {
    let theme = Theme::for_mode(mode);
    if unicode {
        match state {
            StatusState::Ok => format!("{}●{RESET}", theme.accent),
            StatusState::Warn => format!("{}◆{RESET}", theme.yellow),
            StatusState::Alert => format!("{}✖{RESET}", theme.warn),
            StatusState::Inactive => format!("{}○{RESET}", theme.muted),
        }
    } else {
        match state {
            StatusState::Ok => format!("{}*{RESET}", theme.accent),
            StatusState::Warn => format!("{}!{RESET}", theme.yellow),
            StatusState::Alert => format!("{}X{RESET}", theme.warn),
            StatusState::Inactive => format!("{}-{RESET}", theme.muted),
        }
    }
}

// ── Existing functions (preserved) ──────────────────────────────────────────

pub fn mode_banner_adaptive(
    mode: Mode,
    flash_enabled: bool,
    columns: usize,
    unicode_frames: bool,
) -> String {
    let theme = Theme::for_mode(mode.clone());
    let header = match mode {
        Mode::Training => "SOLO TRAINING SIM",
        Mode::NetCity => "MULTIPLAYER NETCITY MMO",
        Mode::Redline => "REDLINE // 5:00 COUNTDOWN",
    };

    let prefix = if mode == Mode::Redline && flash_enabled {
        theme.flash
    } else {
        theme.primary
    };

    if columns < 30 {
        let compact = clipped(header, columns.saturating_sub(6).max(8));
        return format!("{prefix}[ {compact} ]{RESET}");
    }

    let inner_width = columns.saturating_sub(4).clamp(20, 52);
    let title = clipped(header, inner_width);
    if unicode_frames {
        return format!(
            "{prefix}╔{}╗\n║ {:<inner_width$} ║\n╚{}╝\n{RESET}",
            "═".repeat(inner_width + 2),
            title,
            "═".repeat(inner_width + 2),
        );
    }

    format!(
        "{prefix}+{}+\n| {:<inner_width$} |\n+{}+\n{RESET}",
        "-".repeat(inner_width + 2),
        title,
        "-".repeat(inner_width + 2),
    )
}

pub fn mode_banner(mode: Mode, flash_enabled: bool) -> String {
    mode_banner_adaptive(mode, flash_enabled, 80, true)
}

pub fn mode_switch_banner(from: Mode, to: Mode) -> String {
    let text = format!("MODE SWITCH: {} → {}", from.as_label(), to.as_label());
    let theme = Theme::for_mode(to);
    format!("{}{}{}\n", theme.accent, text, RESET)
}

pub fn lore_message(mode: Mode) -> &'static str {
    match mode {
        Mode::Training => {
            "Welcome to CorpSim Onboarding. Ghost Rail is dark, vault-sat-9 is silent, and your tutorial data is pulled from the live outage."
        }
        Mode::NetCity => {
            "NetCity is live. Every district wants the blackout story first, so keep your traces cold and your answers cleaner than the corps."
        }
        Mode::Redline => {
            "REDLINE active. Five minutes. One breach window. Every trace you leave becomes part of the case file."
        }
    }
}

pub fn section_banner_adaptive(
    mode: Mode,
    title: &str,
    columns: usize,
    unicode_frames: bool,
) -> String {
    let theme = Theme::for_mode(mode);
    if columns < 30 {
        let compact = clipped(title, columns.saturating_sub(6).max(8));
        return format!("{}[ {} ]{}\n", theme.primary, compact, RESET);
    }

    let inner_width = columns.saturating_sub(4).clamp(24, 64);
    let title = clipped(title, inner_width);
    if unicode_frames {
        return format!(
            "{primary}┏{}┓\n┃ {:<inner_width$} ┃\n┗{}┛{RESET}\n",
            "━".repeat(inner_width + 2),
            title,
            "━".repeat(inner_width + 2),
            primary = theme.primary,
        );
    }

    format!(
        "{primary}+{}+\n| {:<inner_width$} |\n+{}+{RESET}\n",
        "-".repeat(inner_width + 2),
        title,
        "-".repeat(inner_width + 2),
        primary = theme.primary,
    )
}

pub fn section_banner(mode: Mode, title: &str) -> String {
    section_banner_adaptive(mode, title, 80, true)
}

pub fn key_value_line(mode: Mode, key: &str, value: &str) -> String {
    let theme = Theme::for_mode(mode);
    format!(
        "{}{: <16}{} {}\n",
        theme.accent,
        format!("{key}:"),
        RESET,
        value
    )
}

pub fn progress_meter(mode: Mode, percent: u8, width: usize) -> String {
    let theme = Theme::for_mode(mode);
    let clamped = percent.min(100) as usize;
    let filled = (clamped * width) / 100;
    let empty = width.saturating_sub(filled);
    format!(
        "{accent}{}{muted}{}{reset}",
        "█".repeat(filled),
        "░".repeat(empty),
        accent = theme.accent,
        muted = theme.muted,
        reset = RESET
    )
}

pub fn mission_state_badge(mode: Mode, state: &MissionState) -> String {
    let theme = Theme::for_mode(mode);
    match state {
        MissionState::Locked => format!("{}LOCKED{}", theme.muted, RESET),
        MissionState::Available => format!("{}READY{}", theme.accent, RESET),
        MissionState::Active => format!("{}ACTIVE{}", theme.primary, RESET),
        MissionState::Completed => format!("{}DONE{}", theme.warn, RESET),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── visible_len tests ───────────────────────────────────────────────

    #[test]
    fn visible_len_plain_text() {
        assert_eq!(visible_len("hello"), 5);
    }

    #[test]
    fn visible_len_with_ansi() {
        assert_eq!(visible_len("\x1b[38;5;46mhello\x1b[0m"), 5);
    }

    #[test]
    fn visible_len_empty() {
        assert_eq!(visible_len(""), 0);
    }

    #[test]
    fn visible_len_only_ansi() {
        assert_eq!(visible_len("\x1b[38;5;46m\x1b[0m"), 0);
    }

    #[test]
    fn pad_visible_adds_spaces() {
        let s = "\x1b[38;5;46mhi\x1b[0m";
        let padded = pad_visible(s, 10);
        assert_eq!(visible_len(&padded), 10);
        assert!(padded.ends_with("        ")); // 8 spaces
    }

    // ── Existing tests ──────────────────────────────────────────────────

    #[test]
    fn progress_meter_clamps_overflow() {
        let rendered = progress_meter(Mode::Training, 250, 10);
        assert!(rendered.contains("██████████"));
        assert!(!rendered.contains("░"));
    }

    #[test]
    fn section_banner_contains_title() {
        let rendered = section_banner(Mode::NetCity, "COMMAND MATRIX");
        assert!(rendered.contains("COMMAND MATRIX"));
        assert!(rendered.contains("┏"));
    }

    #[test]
    fn mode_banner_ascii_fallback_uses_ascii_frame() {
        let rendered = mode_banner_adaptive(Mode::Training, false, 80, false);
        assert!(rendered.contains('+'));
        assert!(rendered.contains("SOLO TRAINING SIM"));
    }

    #[test]
    fn section_banner_compacts_on_narrow_width() {
        let rendered = section_banner_adaptive(Mode::NetCity, "MISSION BOARD", 20, true);
        assert!(rendered.contains("[ MISSION BOARD ]"));
    }

    #[test]
    fn mission_state_badges_have_labels() {
        let locked = mission_state_badge(Mode::Training, &MissionState::Locked);
        let done = mission_state_badge(Mode::Training, &MissionState::Completed);
        assert!(locked.contains("LOCKED"));
        assert!(done.contains("DONE"));
    }

    // ── New tests ───────────────────────────────────────────────────────

    #[test]
    fn splash_logo_full_width() {
        let rendered = splash_logo(Mode::Training, 80, true);
        assert!(rendered.contains("███"));
        assert!(rendered.contains("Jack in"));
        assert!(rendered.contains("▓▒░"));
    }

    #[test]
    fn splash_logo_compact() {
        let rendered = splash_logo(Mode::Training, 50, true);
        assert!(rendered.contains("╔═╗╔═╗"));
        assert!(rendered.contains("Jack in"));
    }

    #[test]
    fn splash_logo_narrow() {
        let rendered = splash_logo(Mode::Training, 25, true);
        assert!(rendered.contains("SSH-HUNT"));
    }

    #[test]
    fn splash_logo_narrow_ascii() {
        let rendered = splash_logo(Mode::Training, 25, false);
        assert!(rendered.contains("SSH-HUNT"));
    }

    #[test]
    fn boot_line_ok_contains_tag() {
        let rendered = boot_line(
            Mode::Training,
            "NEURAL LINK",
            "connected",
            BootStatus::Ok,
            80,
        );
        assert!(rendered.contains("[OK]"));
        assert!(rendered.contains("NEURAL LINK"));
        assert!(rendered.contains("connected"));
    }

    #[test]
    fn boot_line_warn_contains_tag() {
        let rendered = boot_line(
            Mode::Training,
            "VAULT-SAT-9",
            "degraded",
            BootStatus::Warn,
            80,
        );
        assert!(rendered.contains("[!!]"));
        assert!(rendered.contains("degraded"));
    }

    #[test]
    fn boot_line_compact_on_narrow() {
        let rendered = boot_line(Mode::Training, "LINK", "ok", BootStatus::Ok, 30);
        assert!(rendered.contains("[OK]"));
        assert!(rendered.contains("LINK"));
    }

    #[test]
    fn boot_lines_align_at_same_width() {
        // Two lines with different label lengths should have same visible width
        let line_a = boot_line(
            Mode::Training,
            "NEURAL LINK",
            "connected",
            BootStatus::Ok,
            80,
        );
        let line_b = boot_line(Mode::Training, "NODE", "online", BootStatus::Ok, 80);
        let vis_a = visible_len(line_a.trim_end());
        let vis_b = visible_len(line_b.trim_end());
        assert_eq!(vis_a, vis_b, "boot lines should have equal visible width");
    }

    #[test]
    fn titled_panel_contains_title() {
        let body = vec!["Line one".to_string(), "Line two".to_string()];
        let rendered = titled_panel(Mode::NetCity, "TEST PANEL", &body, 60, true);
        assert!(rendered.contains("TEST PANEL"));
        assert!(rendered.contains("╔"));
        assert!(rendered.contains("╡"));
        assert!(rendered.contains("Line one"));
        assert!(rendered.contains("Line two"));
        assert!(rendered.contains("╚"));
    }

    #[test]
    fn titled_panel_body_lines_align() {
        let body = vec![
            "short".to_string(),
            "\x1b[38;5;120mcolored text\x1b[0m and plain".to_string(),
        ];
        let rendered = titled_panel(Mode::Training, "TEST", &body, 60, true);
        // All body lines between ║ and ║ should have same visible width
        let body_lines: Vec<&str> = rendered
            .lines()
            .filter(|l| l.contains("║") && !l.contains("╔") && !l.contains("╚"))
            .collect();
        assert!(body_lines.len() >= 2);
        let widths: Vec<usize> = body_lines.iter().map(|l| visible_len(l)).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "all body lines must have same visible width, got {:?}",
            widths
        );
    }

    #[test]
    fn titled_panel_ascii_fallback() {
        let body = vec!["Content".to_string()];
        let rendered = titled_panel(Mode::Training, "TITLE", &body, 50, false);
        assert!(rendered.contains("TITLE"));
        assert!(rendered.contains("+--"));
        assert!(rendered.contains("Content"));
    }

    #[test]
    fn titled_panel_narrow_compact() {
        let body = vec!["Data".to_string()];
        let rendered = titled_panel(Mode::Training, "TITLE", &body, 20, true);
        assert!(rendered.contains("[ TITLE ]"));
        assert!(rendered.contains("Data"));
    }

    #[test]
    fn glitch_divider_unicode() {
        let rendered = glitch_divider(Mode::Training, 40, true);
        assert!(rendered.contains("▓▒░"));
        assert!(rendered.contains("━"));
        assert!(rendered.contains("░▒▓"));
    }

    #[test]
    fn glitch_divider_ascii() {
        let rendered = glitch_divider(Mode::Training, 40, false);
        assert!(rendered.contains("#="));
        assert!(rendered.contains("=#"));
    }

    #[test]
    fn glitch_divider_empty_on_tiny() {
        let rendered = glitch_divider(Mode::Training, 5, true);
        assert!(rendered.is_empty());
    }

    #[test]
    fn two_column_wide() {
        let pairs = vec![("Alias", "hunter-x"), ("Mode", "SIM")];
        let lines = two_column_kv(Mode::Training, &pairs, 80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Alias"));
        assert!(lines[0].contains("Mode"));
    }

    #[test]
    fn two_column_falls_back_narrow() {
        let pairs = vec![("Alias", "hunter-x"), ("Mode", "SIM")];
        let lines = two_column_kv(Mode::Training, &pairs, 40);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn two_column_lines_have_consistent_visible_width() {
        let pairs = vec![
            ("Alias", "hunter-x"),
            ("Mode", "SIM"),
            ("Tier", "Noob"),
            ("Wallet", "500 NC"),
        ];
        let lines = two_column_kv(Mode::Training, &pairs, 80);
        let widths: Vec<usize> = lines.iter().map(|l| visible_len(l)).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "all two_column lines must have same visible width, got {:?}",
            widths
        );
    }

    #[test]
    fn status_dot_all_states_unicode() {
        let ok = status_dot(Mode::Training, StatusState::Ok, true);
        let warn = status_dot(Mode::Training, StatusState::Warn, true);
        let alert = status_dot(Mode::Training, StatusState::Alert, true);
        let inactive = status_dot(Mode::Training, StatusState::Inactive, true);
        assert!(ok.contains("●"));
        assert!(warn.contains("◆"));
        assert!(alert.contains("✖"));
        assert!(inactive.contains("○"));
    }

    #[test]
    fn status_dot_all_states_ascii() {
        let ok = status_dot(Mode::Training, StatusState::Ok, false);
        let warn = status_dot(Mode::Training, StatusState::Warn, false);
        assert!(ok.contains('*'));
        assert!(warn.contains('!'));
    }

    #[test]
    fn neon_header_unicode() {
        let rendered = neon_header(Mode::NetCity, "QUICK START", 60, true);
        assert!(rendered.contains("▸"));
        assert!(rendered.contains("QUICK START"));
        assert!(rendered.contains("◂"));
    }

    #[test]
    fn neon_header_ascii() {
        let rendered = neon_header(Mode::NetCity, "QUICK START", 60, false);
        assert!(rendered.contains(">"));
        assert!(rendered.contains("QUICK START"));
        assert!(rendered.contains("<"));
    }

    #[test]
    fn scanline_unicode() {
        let rendered = scanline(Mode::Training, 40, true);
        assert!(rendered.contains("░"));
    }

    #[test]
    fn scanline_ascii() {
        let rendered = scanline(Mode::Training, 40, false);
        assert!(rendered.contains('-'));
    }
}
