//! Translates HarmonyOS input (delivered by `winit-ohos` from XComponent, ArkUI gestures and the
//! IME) into `egui::RawInput` events.
//!
//! The mapping mirrors what `egui-winit` does on desktop, so every tool, shortcut and drag behaves
//! the same on HarmonyOS.
use dpi::PhysicalPosition;
use egui::{Event, Key, Modifiers, PointerButton, RawInput};
use winit_core::event::{
    ButtonSource, ElementState, Ime, KeyEvent, Modifiers as WinitModifiers, MouseButton as WinitMouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit_core::keyboard::{Key as WinitKey, NamedKey};

/// Collects winit events for the frame that is about to run.
#[derive(Default)]
pub struct Input {
    events: Vec<Event>,
    /// Latest modifier state, stamped onto every event (egui expects them per event).
    modifiers: Modifiers,
    focused: bool,
}

impl Input {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            modifiers: Modifiers::default(),
            // The Ability is foreground while its surface exists; `Focused` events correct this.
            focused: true,
        }
    }

    /// Records one winit window event.
    pub fn handle(&mut self, event: &WindowEvent, pixels_per_point: f32) {
        match event {
            WindowEvent::KeyboardInput { event, .. } => self.key(event),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = to_modifiers(modifiers);
                self.events.push(Event::ModifiersChanged(self.modifiers));
            }
            WindowEvent::PointerMoved { position, .. } => {
                self.events.push(Event::PointerMoved(to_pos(*position, pixels_per_point)));
            }
            WindowEvent::PointerEntered { position, .. } => {
                self.events.push(Event::PointerMoved(to_pos(*position, pixels_per_point)));
            }
            WindowEvent::PointerLeft { .. } => self.events.push(Event::PointerGone),
            WindowEvent::PointerButton { state, position, button, .. } => {
                if let Some(button) = to_pointer_button(button) {
                    self.events.push(Event::PointerButton {
                        pos: to_pos(*position, pixels_per_point),
                        button,
                        pressed: *state == ElementState::Pressed,
                        modifiers: self.modifiers,
                    });
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let mut scroll = egui::Vec2::ZERO;
                let mut unit = egui::MouseWheelUnit::Point;
                match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        unit = egui::MouseWheelUnit::Line;
                        scroll += egui::vec2(*x, *y);
                    }
                    MouseScrollDelta::PixelDelta(delta) => {
                        scroll += egui::vec2(delta.x as f32, delta.y as f32) / pixels_per_point;
                    }
                    // `MouseScrollDelta` is non-exhaustive: future deltas scroll nothing.
                    &_ => {}
                }
                if scroll != egui::Vec2::ZERO {
                    self.events.push(Event::MouseWheel { unit, delta: scroll, phase: to_touch_phase(*phase), modifiers: self.modifiers });
                }
            }
            // HarmonyOS delivers pinch as an ArkUI gesture; egui zooms the canvas from it.
            WindowEvent::PinchGesture { delta, .. } => {
                let delta = *delta as f32;
                if delta.is_finite() && delta != 0.0 {
                    self.events.push(Event::Zoom(delta.clamp(-1.0, 1.0)));
                }
            }
            WindowEvent::PanGesture { delta, .. } => {
                let logical = delta.to_logical::<f32>(f64::from(pixels_per_point));
                let delta = egui::vec2(logical.x, logical.y);
                if delta != egui::Vec2::ZERO {
                    self.events.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, phase: egui::TouchPhase::Move, modifiers: self.modifiers });
                }
            }
            WindowEvent::Ime(ime) => match ime {
                Ime::Commit(text) => {
                    if !text.is_empty() {
                        self.events.push(Event::Text(text.clone()));
                    }
                }
                Ime::Preedit(text, cursor) => {
                    let (start, end) = cursor.unwrap_or((0, 0));
                    self.events.push(Event::Ime(egui::ImeEvent::Preedit { text: text.clone(), active_range_chars: Some(start..end) }));
                }
                _ => {}
            },
            WindowEvent::Focused(focused) => {
                self.focused = *focused;
                self.events.push(Event::WindowFocused(*focused));
            }
            _ => {}
        }
    }

    /// Builds the input for the next `egui` frame.
    pub fn take(&mut self, screen_rect: egui::Rect, dt: std::time::Duration, pixels_per_point: f32) -> RawInput {
        let mut viewports = egui::ViewportIdMap::default();
        viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { native_pixels_per_point: Some(pixels_per_point), ..Default::default() });
        RawInput {
            screen_rect: Some(screen_rect),
            viewports,
            time: Some(frame_time_seconds()),
            predicted_dt: dt.as_secs_f32(),
            events: std::mem::take(&mut self.events),
            focused: self.focused,
            ..Default::default()
        }
    }

    fn key(&mut self, event: &KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        // Text first: this is what makes the type tool work with non-Latin layouts.
        if pressed
            && let Some(text) = event.text.as_ref()
            && !text.is_empty()
        {
            self.events.push(Event::Text(text.to_string()));
        }
        // Character keys double as the shortcut keys egui matches (Ctrl+C is `Key::C`); keys with
        // no egui name (dead keys, multi-char compositions) are text-only.
        if let Some(key) = to_key(&event.logical_key) {
            self.events.push(Event::Key { key, physical_key: None, pressed, repeat: event.repeat && pressed, modifiers: self.modifiers });
        }
    }
}

/// HarmonyOS sends physical keys through ArkUI; `egui` wants the logical character.
fn to_key(key: &WinitKey) -> Option<Key> {
    match key {
        WinitKey::Character(text) => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(single), None) => char_key(single),
                _ => None,
            }
        }
        WinitKey::Named(named) => Some(match named {
            NamedKey::Enter => Key::Enter,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Tab => Key::Tab,
            // Space arrives as `Character(" ")`, not as a named key.
            NamedKey::Escape => Key::Escape,
            NamedKey::ArrowLeft => Key::ArrowLeft,
            NamedKey::ArrowRight => Key::ArrowRight,
            NamedKey::ArrowUp => Key::ArrowUp,
            NamedKey::ArrowDown => Key::ArrowDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Delete => Key::Delete,
            NamedKey::Insert => Key::Insert,
            NamedKey::Shift => Key::ShiftLeft,
            NamedKey::Control => Key::ControlLeft,
            NamedKey::Alt => Key::AltLeft,
            // `Super` is the legacy name for `Meta` in the UI Events spec.
            NamedKey::Meta => Key::SuperLeft,
            NamedKey::F1 => Key::F1,
            NamedKey::F2 => Key::F2,
            NamedKey::F3 => Key::F3,
            NamedKey::F4 => Key::F4,
            NamedKey::F5 => Key::F5,
            NamedKey::F6 => Key::F6,
            NamedKey::F7 => Key::F7,
            NamedKey::F8 => Key::F8,
            NamedKey::F9 => Key::F9,
            NamedKey::F10 => Key::F10,
            NamedKey::F11 => Key::F11,
            NamedKey::F12 => Key::F12,
            _ => return None,
        }),
        _ => None,
    }
}

/// Names a printable character the way egui's shortcut matcher expects (`ctrl` + `c` is `Key::C`).
fn char_key(character: char) -> Option<Key> {
    Some(match character.to_ascii_uppercase() {
        'A' => Key::A,
        'B' => Key::B,
        'C' => Key::C,
        'D' => Key::D,
        'E' => Key::E,
        'F' => Key::F,
        'G' => Key::G,
        'H' => Key::H,
        'I' => Key::I,
        'J' => Key::J,
        'K' => Key::K,
        'L' => Key::L,
        'M' => Key::M,
        'N' => Key::N,
        'O' => Key::O,
        'P' => Key::P,
        'Q' => Key::Q,
        'R' => Key::R,
        'S' => Key::S,
        'T' => Key::T,
        'U' => Key::U,
        'V' => Key::V,
        'W' => Key::W,
        'X' => Key::X,
        'Y' => Key::Y,
        'Z' => Key::Z,
        '0' => Key::Num0,
        '1' => Key::Num1,
        '2' => Key::Num2,
        '3' => Key::Num3,
        '4' => Key::Num4,
        '5' => Key::Num5,
        '6' => Key::Num6,
        '7' => Key::Num7,
        '8' => Key::Num8,
        '9' => Key::Num9,
        ',' => Key::Comma,
        '.' => Key::Period,
        '/' => Key::Slash,
        ';' => Key::Semicolon,
        '\'' => Key::Quote,
        '[' => Key::OpenBracket,
        ']' => Key::CloseBracket,
        '\\' => Key::Backslash,
        '-' => Key::Minus,
        '=' => Key::Equals,
        '`' => Key::Backtick,
        ':' => Key::Colon,
        '?' => Key::Questionmark,
        '!' => Key::Exclamationmark,
        '|' => Key::Pipe,
        '+' => Key::Plus,
        '{' => Key::OpenCurlyBracket,
        '}' => Key::CloseCurlyBracket,
        ' ' => Key::Space,
        _ => return None,
    })
}

fn to_pointer_button(button: &ButtonSource) -> Option<PointerButton> {
    match button {
        ButtonSource::Mouse(button) => match button {
            WinitMouseButton::Left => Some(PointerButton::Primary),
            WinitMouseButton::Right => Some(PointerButton::Secondary),
            WinitMouseButton::Middle => Some(PointerButton::Middle),
            WinitMouseButton::Back => Some(PointerButton::Extra1),
            WinitMouseButton::Forward => Some(PointerButton::Extra2),
            _ => None,
        },
        // A finger on the canvas behaves like the primary button, which is what the tools expect.
        ButtonSource::Touch { .. } | ButtonSource::TabletTool { .. } => Some(PointerButton::Primary),
        // The backend reports `NoneButton` (unknown source) for presses whose button it could not
        // classify, including injected ones: a press with no button information is still a press.
        ButtonSource::Unknown(_) => Some(PointerButton::Primary),
        // `ButtonSource` is non-exhaustive: future sources map to nothing.
        &_ => None,
    }
}

fn to_modifiers(modifiers: &WinitModifiers) -> Modifiers {
    let state = modifiers.state();
    let command = state.meta_key();
    Modifiers {
        alt: state.alt_key(),
        ctrl: state.control_key(),
        shift: state.shift_key(),
        // HarmonyOS has no macOS-style separate command key; Meta acts as the command modifier.
        mac_cmd: false,
        command,
    }
}

fn to_touch_phase(phase: TouchPhase) -> egui::TouchPhase {
    match phase {
        TouchPhase::Started => egui::TouchPhase::Start,
        TouchPhase::Moved => egui::TouchPhase::Move,
        TouchPhase::Ended => egui::TouchPhase::End,
        TouchPhase::Cancelled => egui::TouchPhase::Cancel,
    }
}

fn to_pos(position: PhysicalPosition<f64>, pixels_per_point: f32) -> egui::Pos2 {
    let logical = position.to_logical::<f64>(f64::from(pixels_per_point));
    egui::pos2(logical.x as f32, logical.y as f32)
}

/// egui wants seconds since start; monotonic is enough.
fn frame_time_seconds() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit_core::event::{Modifiers as WinitModifiers, MouseButton as WinitMouseButton};
    use winit_core::keyboard::{ModifiersKeys, ModifiersState};

    fn character(text: &str) -> WinitKey {
        WinitKey::Character(text.into())
    }

    #[test]
    fn letters_digits_and_punctuation_map_to_shortcut_keys() {
        assert_eq!(to_key(&character("a")), Some(Key::A));
        assert_eq!(to_key(&character("Z")), Some(Key::Z));
        assert_eq!(to_key(&character("0")), Some(Key::Num0));
        assert_eq!(to_key(&character("9")), Some(Key::Num9));
        assert_eq!(to_key(&character(",")), Some(Key::Comma));
        assert_eq!(to_key(&character(" ")), Some(Key::Space));
    }

    #[test]
    fn multi_char_compositions_have_no_key_event() {
        assert_eq!(to_key(&character("ch")), None);
        assert_eq!(to_key(&character("ß")), None);
    }

    #[test]
    fn named_keys_map_to_egui_keys() {
        assert_eq!(to_key(&WinitKey::Named(NamedKey::Enter)), Some(Key::Enter));
        assert_eq!(to_key(&WinitKey::Named(NamedKey::Escape)), Some(Key::Escape));
        assert_eq!(to_key(&WinitKey::Named(NamedKey::ArrowLeft)), Some(Key::ArrowLeft));
        assert_eq!(to_key(&WinitKey::Named(NamedKey::Shift)), Some(Key::ShiftLeft));
        assert_eq!(to_key(&WinitKey::Named(NamedKey::Control)), Some(Key::ControlLeft));
        assert_eq!(to_key(&WinitKey::Named(NamedKey::Alt)), Some(Key::AltLeft));
        assert_eq!(to_key(&WinitKey::Named(NamedKey::Meta)), Some(Key::SuperLeft));
        assert_eq!(to_key(&WinitKey::Named(NamedKey::F5)), Some(Key::F5));
        assert_eq!(to_key(&WinitKey::Named(NamedKey::Delete)), Some(Key::Delete));
    }

    #[test]
    fn mouse_touch_and_unknown_buttons_become_primary() {
        use winit_core::event::ButtonSource;
        assert_eq!(to_pointer_button(&ButtonSource::Mouse(WinitMouseButton::Left)), Some(PointerButton::Primary));
        assert_eq!(to_pointer_button(&ButtonSource::Mouse(WinitMouseButton::Right)), Some(PointerButton::Secondary));
        assert_eq!(to_pointer_button(&ButtonSource::Mouse(WinitMouseButton::Middle)), Some(PointerButton::Middle));
        // Presses the backend could not classify (e.g. injected ones) still count as presses.
        assert_eq!(to_pointer_button(&ButtonSource::Unknown(0)), Some(PointerButton::Primary));
    }

    #[test]
    fn modifier_state_maps_to_egui_modifiers() {
        let mods = to_modifiers(&WinitModifiers::new(ModifiersState::SHIFT, ModifiersKeys::empty()));
        assert!(mods.shift && !mods.ctrl && !mods.alt && !mods.command);
        let mods = to_modifiers(&WinitModifiers::new(ModifiersState::CONTROL.union(ModifiersState::META), ModifiersKeys::empty()));
        assert!(mods.ctrl && mods.command && !mods.shift);
    }

    #[test]
    fn physical_positions_scale_by_pixels_per_point() {
        let pos = to_pos(PhysicalPosition::new(190.0, 95.0), 2.0);
        assert_eq!(pos, egui::pos2(95.0, 47.5));
    }

    #[test]
    fn frame_clock_is_monotonic() {
        assert!(frame_time_seconds() <= frame_time_seconds());
    }
}
