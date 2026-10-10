use glib::{ControlFlow, EnumValue};
use gtk4::cairo::{Context, LineCap};
use gtk4::prelude::*;
use gtk4::{AccessibleRole, Align, DrawingArea};
use std::f64::consts::{FRAC_PI_2, TAU};

pub fn create_spinner(size: i32) -> DrawingArea {
    let spinner = DrawingArea::builder()
        .content_width(size)
        .content_height(size)
        .halign(Align::Center)
        .valign(Align::Center)
        .accessible_role(AccessibleRole::ProgressBar)
        .build();
    spinner.set_draw_func(draw_spinner);
    spinner.add_tick_callback(|spinner, _| {
        if animations_enabled(spinner) {
            spinner.queue_draw();
        }
        return ControlFlow::Continue;
    });
    return spinner;
}

fn animations_enabled(spinner: &DrawingArea) -> bool {
    let settings = spinner.settings();
    if !settings.is_gtk_enable_animations() {
        return false;
    }
    if settings
        .find_property("gtk-interface-reduced-motion")
        .is_some()
    {
        let value = settings.property_value("gtk-interface-reduced-motion");
        if let Some((_, motion)) = EnumValue::from_value(&value) {
            return motion.value() == 0;
        }
    }
    return true;
}

fn draw_spinner(spinner: &DrawingArea, context: &Context, width: i32, height: i32) {
    let color = spinner.color();
    let red = f64::from(color.red());
    let green = f64::from(color.green());
    let blue = f64::from(color.blue());
    let alpha = f64::from(color.alpha());
    let size = f64::from(width.min(height));
    let radius = size * 7.0 / 16.0;
    let center_x = f64::from(width) / 2.0;
    let center_y = f64::from(height) / 2.0;
    let mut angle = -FRAC_PI_2;
    if animations_enabled(spinner) {
        if let Some(clock) = spinner.frame_clock() {
            angle += (clock.frame_time() % 1_000_000) as f64 / 1_000_000.0 * TAU;
        }
    }
    context.set_line_width(size / 8.0);
    context.set_line_cap(LineCap::Round);
    context.set_source_rgba(red, green, blue, alpha * 0.2);
    context.arc(center_x, center_y, radius, 0.0, TAU);
    let _ = context.stroke();
    context.set_source_rgba(red, green, blue, alpha);
    context.arc(center_x, center_y, radius, angle, angle + FRAC_PI_2);
    let _ = context.stroke();
    return;
}
