#![forbid(unsafe_code)]

use protocol::{MissionState, Mode};

pub const RESET: &str = "\x1b[0m";

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub primary: &'static str,
    pub accent: &'static str,
    pub warn: &'static str,
    pub flash: &'static str,
    pub muted: &'static str,
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
            },
            Mode::NetCity => Self {
                primary: "\x1b[38;5;141m",
                accent: "\x1b[38;5;213m",
                warn: "\x1b[38;5;177m",
                flash: "\x1b[48;5;53m\x1b[38;5;213m",
                muted: "\x1b[38;5;245m",
            },
            Mode::Redline => Self {
                primary: "\x1b[38;5;196m",
                accent: "\x1b[38;5;203m",
                warn: "\x1b[38;5;210m",
                flash: "\x1b[5m\x1b[38;5;15m\x1b[48;5;160m",
                muted: "\x1b[38;5;245m",
            },
        }
    }
}

pub fn mode_banner(mode: Mode, flash_enabled: bool) -> String {
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

    format!(
        "{prefix}\n╔══════════════════════════════════════╗\n║ {:<36} ║\n╚══════════════════════════════════════╝\n{RESET}",
        header
    )
}

pub fn mode_switch_banner(from: Mode, to: Mode) -> String {
    let text = format!("MODE SWITCH: {} → {}", from.as_label(), to.as_label());
    let theme = Theme::for_mode(to);
    format!("{}{}{}\n", theme.accent, text, RESET)
}

pub fn lore_message(mode: Mode) -> &'static str {
    match mode {
        Mode::Training => "Welcome to CorpSim Onboarding. This simulation recruits real hunters.",
        Mode::NetCity => "NetCity is live. Megacorps are watching. Keep your traces cold.",
        Mode::Redline => "REDLINE active. Five minutes. One breach window. No hesitation.",
    }
}

pub fn section_banner(mode: Mode, title: &str) -> String {
    let theme = Theme::for_mode(mode);
    format!(
        "{primary}┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓\n┃ {title:<52} ┃\n┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛{RESET}\n",
        primary = theme.primary,
        title = title
    )
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn mission_state_badges_have_labels() {
        let locked = mission_state_badge(Mode::Training, &MissionState::Locked);
        let done = mission_state_badge(Mode::Training, &MissionState::Completed);
        assert!(locked.contains("LOCKED"));
        assert!(done.contains("DONE"));
    }
}
