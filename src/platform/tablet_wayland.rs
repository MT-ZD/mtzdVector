//! Drawing tablets on Wayland (tablet-v2), on the drag-and-drop worker's
//! connection. winit reports pens only as an emulated mouse, without
//! pressure. Once a client binds the tablet seat, the compositor stops that
//! emulation for it, so pen motion and clicks are forwarded as `PenEvent`s
//! and fed to egui as pointer input (`App::raw_input_hook`), with pressure
//! alongside.

use wayland_client::protocol::wl_seat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::wp::cursor_shape::v1::client::{wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1};
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2, zwp_tablet_pad_group_v2, zwp_tablet_pad_ring_v2, zwp_tablet_pad_strip_v2, zwp_tablet_pad_v2, zwp_tablet_seat_v2, zwp_tablet_tool_v2, zwp_tablet_v2,
};

use super::PenEvent;
use super::dnd_wayland::State;

/// Pen state between `frame` events.
#[derive(Default)]
pub struct ToolState {
    pub tool: Option<zwp_tablet_tool_v2::ZwpTabletToolV2>,
    pub cursor: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    /// Over our window (between proximity in and out).
    pub on_surface: bool,
    pub pending: Vec<PenEvent>,
}

/// Bind the tablet seat for every seat (when the compositor has tablet-v2).
pub(super) fn bind(globals: &wayland_client::globals::GlobalList, qh: &QueueHandle<State>, seats: &[wl_seat::WlSeat]) -> (Option<zwp_tablet_manager_v2::ZwpTabletManagerV2>, Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>) {
    let Ok(manager) = globals.bind::<zwp_tablet_manager_v2::ZwpTabletManagerV2, _, _>(qh, 1..=1, ()) else { return (None, None) };
    for seat in seats {
        manager.get_tablet_seat(seat, qh, ());
    }
    let cursor = globals.bind::<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, _, _>(qh, 1..=1, ()).ok();
    (Some(manager), cursor)
}

/// Pen events as egui pointer input (`zf`: egui zoom factor, surface px → points).
pub fn to_egui(events: &[PenEvent], zf: f32, modifiers: egui::Modifiers, last: &mut egui::Pos2) -> Vec<egui::Event> {
    let mut out = Vec::new();
    for e in events {
        match *e {
            PenEvent::Motion(p) => {
                *last = egui::pos2(p.x / zf, p.y / zf);
                out.push(egui::Event::PointerMoved(*last));
            }
            PenEvent::Button { button, pressed } => out.push(egui::Event::PointerButton { pos: *last, button, pressed, modifiers }),
            PenEvent::Leave => out.push(egui::Event::PointerGone),
            PenEvent::Pressure(_) => {}
        }
    }
    out
}

impl Dispatch<zwp_tablet_manager_v2::ZwpTabletManagerV2, ()> for State {
    fn event(_: &mut Self, _: &zwp_tablet_manager_v2::ZwpTabletManagerV2, _: zwp_tablet_manager_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for State {
    fn event(_: &mut Self, _: &wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, _: wp_cursor_shape_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, ()> for State {
    fn event(_: &mut Self, _: &wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, _: wp_cursor_shape_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zwp_tablet_seat_v2::ZwpTabletSeatV2, ()> for State {
    fn event(state: &mut Self, _: &zwp_tablet_seat_v2::ZwpTabletSeatV2, event: zwp_tablet_seat_v2::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let zwp_tablet_seat_v2::Event::ToolAdded { id } = event {
            let cursor = state.cursor_shapes.as_ref().map(|m| m.get_tablet_tool_v2(&id, qh, ()));
            state.tools.push(ToolState { tool: Some(id), cursor, ..Default::default() });
        }
    }

    event_created_child!(State, zwp_tablet_seat_v2::ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (zwp_tablet_v2::ZwpTabletV2, ()),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (zwp_tablet_tool_v2::ZwpTabletToolV2, ()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (zwp_tablet_pad_v2::ZwpTabletPadV2, ()),
    ]);
}

impl Dispatch<zwp_tablet_v2::ZwpTabletV2, ()> for State {
    fn event(_: &mut Self, tablet: &zwp_tablet_v2::ZwpTabletV2, event: zwp_tablet_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwp_tablet_v2::Event::Removed = event {
            tablet.destroy();
        }
    }
}

/// evdev stylus buttons.
const BTN_STYLUS: u32 = 0x14b;
const BTN_STYLUS2: u32 = 0x14c;

impl Dispatch<zwp_tablet_tool_v2::ZwpTabletToolV2, ()> for State {
    fn event(state: &mut Self, tool: &zwp_tablet_tool_v2::ZwpTabletToolV2, event: zwp_tablet_tool_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_tablet_tool_v2::Event;
        let surface = state.surface;
        let Some(i) = state.tools.iter().position(|t| t.tool.as_ref() == Some(tool)) else { return };
        let t = &mut state.tools[i];
        match event {
            Event::ProximityIn { serial, surface: s, .. } => {
                t.on_surface = s.id().as_ptr() as usize == surface;
                if let Some(c) = &t.cursor {
                    c.set_shape(serial, wp_cursor_shape_device_v1::Shape::Default);
                }
            }
            Event::ProximityOut => {
                if t.on_surface {
                    t.pending.push(PenEvent::Leave);
                }
                t.on_surface = false;
            }
            Event::Frame { .. } => {
                let batch = std::mem::take(&mut t.pending);
                if !batch.is_empty() {
                    for e in batch {
                        let _ = state.pen_tx.send(e);
                    }
                    state.ctx.request_repaint();
                }
            }
            Event::Removed => {
                if let Some(c) = t.cursor.take() {
                    c.destroy();
                }
                tool.destroy();
                state.tools.remove(i);
            }
            _ if !t.on_surface => {}
            Event::Motion { x, y } => t.pending.push(PenEvent::Motion(egui::pos2(x as f32, y as f32))),
            Event::Pressure { pressure } => t.pending.push(PenEvent::Pressure(pressure as f32 / 65535.0)),
            Event::Down { .. } => t.pending.push(PenEvent::Button { button: egui::PointerButton::Primary, pressed: true }),
            Event::Up => t.pending.push(PenEvent::Button { button: egui::PointerButton::Primary, pressed: false }),
            Event::Button { button, state: WEnum::Value(st), .. } => {
                let b = match button {
                    BTN_STYLUS => egui::PointerButton::Secondary,
                    BTN_STYLUS2 => egui::PointerButton::Middle,
                    _ => return,
                };
                t.pending.push(PenEvent::Button { button: b, pressed: st == zwp_tablet_tool_v2::ButtonState::Pressed });
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_tablet_pad_v2::ZwpTabletPadV2, ()> for State {
    fn event(_: &mut Self, pad: &zwp_tablet_pad_v2::ZwpTabletPadV2, event: zwp_tablet_pad_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwp_tablet_pad_v2::Event::Removed = event {
            pad.destroy();
        }
    }

    event_created_child!(State, zwp_tablet_pad_v2::ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2, ()),
    ]);
}

impl Dispatch<zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2, ()> for State {
    fn event(_: &mut Self, _: &zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2, _: zwp_tablet_pad_group_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}

    event_created_child!(State, zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2, ()),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2, ()),
    ]);
}

impl Dispatch<zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2, ()> for State {
    fn event(_: &mut Self, _: &zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2, _: zwp_tablet_pad_ring_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2, ()> for State {
    fn event(_: &mut Self, _: &zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2, _: zwp_tablet_pad_strip_v2::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pen_events_become_pointer_input() {
        let ev = [
            PenEvent::Motion(egui::pos2(100.0, 50.0)),
            PenEvent::Pressure(0.5),
            PenEvent::Button { button: egui::PointerButton::Primary, pressed: true },
            PenEvent::Motion(egui::pos2(120.0, 60.0)),
            PenEvent::Button { button: egui::PointerButton::Primary, pressed: false },
            PenEvent::Leave,
        ];
        let mut last = egui::Pos2::ZERO;
        let out = to_egui(&ev, 2.0, egui::Modifiers::NONE, &mut last);
        assert_eq!(out.len(), 5);
        assert!(matches!(out[0], egui::Event::PointerMoved(p) if p == egui::pos2(50.0, 25.0)));
        assert!(matches!(out[1], egui::Event::PointerButton { pos, pressed: true, .. } if pos == egui::pos2(50.0, 25.0)));
        assert!(matches!(out[3], egui::Event::PointerButton { pos, pressed: false, .. } if pos == egui::pos2(60.0, 30.0)));
        assert!(matches!(out[4], egui::Event::PointerGone));
    }
}
