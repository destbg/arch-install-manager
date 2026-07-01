use std::io::{Write, stdout};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{Event, KeyCode, KeyModifiers, read};
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
};
use crossterm::{execute, queue};

use crate::models::package_source::PackageSource;
use crate::models::package_update::PackageUpdate;

pub fn pick_updates(items: &[PackageUpdate]) -> Option<Vec<usize>> {
    if items.is_empty() {
        return Some(Vec::new());
    }

    if enable_raw_mode().is_err() {
        return None;
    }
    if execute!(stdout(), EnterAlternateScreen, Hide).is_err() {
        let _ = disable_raw_mode();
        return None;
    }

    let result = run_picker(items);

    let _ = execute!(stdout(), Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();

    return result;
}

fn run_picker(items: &[PackageUpdate]) -> Option<Vec<usize>> {
    let mut selected = vec![true; items.len()];
    let mut cursor = 0usize;
    let mut offset = 0usize;

    loop {
        draw(items, &selected, cursor, &mut offset);

        let Ok(event) = read() else {
            return None;
        };
        let Event::Key(key) = event else {
            continue;
        };

        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return None,
            KeyCode::Esc | KeyCode::Char('q') => return None,
            KeyCode::Enter => {
                let picks: Vec<usize> = selected
                    .iter()
                    .enumerate()
                    .filter(|(_, on)| **on)
                    .map(|(i, _)| i)
                    .collect();
                return Some(picks);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if cursor > 0 {
                    cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if cursor + 1 < items.len() {
                    cursor += 1;
                }
            }
            KeyCode::Char(' ') | KeyCode::Tab => {
                selected[cursor] = !selected[cursor];
            }
            KeyCode::Char('a') => {
                let all_on = selected.iter().all(|on| *on);
                for on in selected.iter_mut() {
                    *on = !all_on;
                }
            }
            _ => {}
        }
    }
}

fn draw(items: &[PackageUpdate], selected: &[bool], cursor: usize, offset: &mut usize) {
    let (cols, rows) = size().unwrap_or((80, 24));
    let cols = cols as usize;
    let visible = (rows as usize).saturating_sub(4).max(1);

    if cursor < *offset {
        *offset = cursor;
    } else if cursor >= *offset + visible {
        *offset = cursor + 1 - visible;
    }

    let count = selected.iter().filter(|on| **on).count();
    let mut out = stdout();
    let _ = queue!(out, Clear(ClearType::All), MoveTo(0, 0));

    let header = "Select updates to install";
    let _ = queue!(
        out,
        SetAttribute(Attribute::Bold),
        Print(header),
        SetAttribute(Attribute::Reset)
    );
    let _ = queue!(out, MoveTo(0, 1));
    let _ = queue!(
        out,
        Print("space/tab toggle, a all, up/down move, enter install, q cancel")
    );

    let end = (*offset + visible).min(items.len());
    for (line, i) in (*offset..end).enumerate() {
        let _ = queue!(out, MoveTo(0, line as u16 + 3));
        let mark = if selected[i] { "[x]" } else { "[ ]" };
        let text = truncate(&format!("{} {}", mark, format_row(&items[i])), cols);
        if i == cursor {
            let _ = queue!(
                out,
                SetAttribute(Attribute::Reverse),
                Print(text),
                SetAttribute(Attribute::Reset)
            );
        } else {
            let _ = queue!(out, Print(text));
        }
    }

    let _ = queue!(out, MoveTo(0, rows.saturating_sub(1)));
    let _ = queue!(out, Print(format!("{} of {} selected", count, items.len())));
    let _ = out.flush();
}

fn format_row(pkg: &PackageUpdate) -> String {
    let repo = if pkg.source == PackageSource::Aur {
        "aur".to_string()
    } else if pkg.repository.is_empty() {
        "repo".to_string()
    } else {
        pkg.repository.clone()
    };
    if pkg.current_version.is_empty() {
        return format!("{}/{}  {}", repo, pkg.name, pkg.new_version);
    }
    return format!(
        "{}/{}  {} -> {}",
        repo, pkg.name, pkg.current_version, pkg.new_version
    );
}

fn truncate(text: &str, max: usize) -> String {
    return text.chars().take(max).collect();
}
