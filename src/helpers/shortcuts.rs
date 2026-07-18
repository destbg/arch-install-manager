use gtk4::gdk::{Key, ModifierType};

use crate::models::app_settings::{
    AppSettings, default_shortcut_focus_search, default_shortcut_install_tab,
    default_shortcut_manage_tab, default_shortcut_update_tab,
};

pub const SHORTCUT_COUNT: usize = 4;

pub fn shortcut_label(index: usize) -> &'static str {
    return match index {
        0 => "Switch to the Install tab",
        1 => "Switch to the Update tab",
        2 => "Switch to the Manage tab",
        _ => "Focus the search bar",
    };
}

pub fn shortcut_default(index: usize) -> String {
    return match index {
        0 => default_shortcut_install_tab(),
        1 => default_shortcut_update_tab(),
        2 => default_shortcut_manage_tab(),
        _ => default_shortcut_focus_search(),
    };
}

pub fn shortcut_get(settings: &AppSettings, index: usize) -> &str {
    return match index {
        0 => &settings.shortcut_install_tab,
        1 => &settings.shortcut_update_tab,
        2 => &settings.shortcut_manage_tab,
        _ => &settings.shortcut_focus_search,
    };
}

pub fn shortcut_set(settings: &mut AppSettings, index: usize, accel: &str) {
    match index {
        0 => settings.shortcut_install_tab = accel.to_string(),
        1 => settings.shortcut_update_tab = accel.to_string(),
        2 => settings.shortcut_manage_tab = accel.to_string(),
        _ => settings.shortcut_focus_search = accel.to_string(),
    }
    return;
}

pub fn shortcut_display(settings: &AppSettings, index: usize) -> String {
    let accel = shortcut_get(settings, index);
    if accel.is_empty() {
        return "Disabled".to_string();
    }
    let Some((key, mods)) = gtk4::accelerator_parse(accel) else {
        return accel.to_string();
    };
    return gtk4::accelerator_get_label(key, mods).to_string();
}

pub fn shortcut_matches(accel: &str, keyval: Key, state: ModifierType) -> bool {
    if accel.is_empty() {
        return false;
    }
    let Some((key, mods)) = gtk4::accelerator_parse(accel) else {
        return false;
    };
    let pressed = state & gtk4::accelerator_get_default_mod_mask();
    return key.to_lower() == keyval.to_lower() && mods == pressed;
}

pub fn accels_equal(first: &str, second: &str) -> bool {
    if first.is_empty() || second.is_empty() {
        return false;
    }
    let parsed_first = gtk4::accelerator_parse(first);
    let parsed_second = gtk4::accelerator_parse(second);
    return match (parsed_first, parsed_second) {
        (Some((key_first, mods_first)), Some((key_second, mods_second))) => {
            key_first.to_lower() == key_second.to_lower() && mods_first == mods_second
        }
        _ => first == second,
    };
}

pub fn is_modifier_key(keyval: Key) -> bool {
    return matches!(
        keyval,
        Key::Shift_L
            | Key::Shift_R
            | Key::Control_L
            | Key::Control_R
            | Key::Alt_L
            | Key::Alt_R
            | Key::Super_L
            | Key::Super_R
            | Key::Meta_L
            | Key::Meta_R
            | Key::ISO_Level3_Shift
            | Key::Caps_Lock
            | Key::Shift_Lock
    );
}
