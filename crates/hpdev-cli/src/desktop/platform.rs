//! Native lifecycle and accelerator policy; execution stays in hpdev-core.
use super::upstream::scaler::Viewport;
use winit::{
    event_loop::{EventLoop, EventLoopBuilder},
    keyboard::{Key, ModifiersState},
};

pub fn event_loop() -> EventLoopBuilder<()> {
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        // This is called on main(), never on the supervisor or VM threads.
        builder
            .with_activation_policy(ActivationPolicy::Regular)
            .with_activate_ignoring_other_apps(true)
            .with_default_menu(false);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = &mut builder;
    builder
}

pub fn close_shortcut(key: &Key, modifiers: ModifiersState) -> bool {
    cfg!(target_os = "macos")
        && modifiers.super_key()
        && matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("q") || c.eq_ignore_ascii_case("w"))
}
pub fn command_modifier(modifiers: ModifiersState) -> bool {
    cfg!(target_os = "macos") && modifiers.super_key()
}
pub fn text_allowed(modifiers: ModifiersState) -> bool {
    !modifiers.control_key()
        && !modifiers.super_key()
        && (cfg!(target_os = "macos") || !modifiers.alt_key())
}
pub fn pixels(points: u32, dpi: f64) -> u32 {
    (f64::from(points) * dpi).round() as u32
}
pub fn viewport(width: u32, height: u32, dpi: f64, sidebar: u32) -> Viewport {
    let bar = pixels(super::BAR, dpi);
    let mut viewport = Viewport::calculate_integer(
        width.saturating_sub(pixels(sidebar, dpi)),
        height.saturating_sub(bar),
    );
    viewport.y += bar;
    viewport
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_pixels_survive_retina_fractional_dpi_and_panel_changes() {
        for dpi in [1., 1.25, 1.5, 2., 3.] {
            for sidebar in [0, 260, 440, 700] {
                let width = pixels(640 + sidebar, dpi);
                let height = pixels(512, dpi);
                let vp = viewport(width, height, dpi, sidebar);
                assert!(vp.is_integer);
                assert_eq!((vp.width, vp.height), (320 * vp.scale, 240 * vp.scale));
                assert!(vp.x + vp.width <= width - pixels(sidebar, dpi));
                assert!(vp.y >= pixels(32, dpi) && vp.y + vp.height <= height);
                assert_eq!(
                    super::super::upstream::scaler::window_to_framebuffer(
                        f64::from(vp.x + vp.width / 2),
                        f64::from(vp.y + vp.height / 2),
                        &vp
                    ),
                    Some((160, 120))
                );
            }
        }
    }
}
