//! Wayland drag & drop (winit 0.30 only implements file drops on X11).
//!
//! We wrap eframe's foreign `wl_display`, create our own event queue on a
//! worker thread, bind `wl_seat` + `wl_data_device_manager`, and act as a
//! drop target for our window's `wl_surface`. The same seat is also used to
//! watch Ctrl+C/X/V on a `wl_keyboard`, because egui-winit swallows those keys
//! and only reports a paste when the clipboard holds plain text.

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Read;
use std::os::fd::AsFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_data_device, wl_data_device_manager, wl_data_offer, wl_keyboard, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};

use super::{ClipKey, PlatformEvent};

const ACCEPT: [&str; 6] = ["text/uri-list", "image/svg+xml", "image/png", "image/jpeg", "image/webp", "image/gif"];

struct Seat {
    seat: wl_seat::WlSeat,
    device: Option<wl_data_device::WlDataDevice>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
}

pub(super) struct State {
    conn: Connection,
    pub(super) surface: usize,
    manager: wl_data_device_manager::WlDataDeviceManager,
    manager_version: u32,
    seats: Vec<Seat>,
    offers: HashMap<ObjectId, Vec<String>>,
    /// Offer currently hovering our surface: (offer, enter serial, accepted mime).
    current: Option<(wl_data_offer::WlDataOffer, u32, Option<String>)>,
    selection: Option<wl_data_offer::WlDataOffer>,
    pos: egui::Pos2,
    mods: u32,
    keyboard_focus: bool,
    keyboard_flag: Arc<AtomicBool>,
    tx: Sender<PlatformEvent>,
    pub(super) ctx: egui::Context,
    // Drawing tablets (tablet-v2), when enabled.
    pub(super) tools: Vec<super::tablet_wayland::ToolState>,
    pub(super) cursor_shapes: Option<wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    pub(super) pen_tx: Sender<super::PenEvent>,
}

impl State {
    fn send(&self, e: PlatformEvent) {
        let _ = self.tx.send(e);
        self.ctx.request_repaint();
    }
}

/// Start the worker. `display` / `surface` are the raw `wl_display*` and
/// `wl_surface*` of eframe's window.
/// With `tablet`, pens are read through tablet-v2 (with pressure) and
/// reported on the second receiver.
pub fn start(display: *mut c_void, surface: *mut c_void, ctx: egui::Context, keyboard_flag: Arc<AtomicBool>, tablet: bool) -> Result<(Receiver<PlatformEvent>, Option<Receiver<super::PenEvent>>), String> {
    // SAFETY: the display pointer comes from the live eframe window and stays
    // valid for the life of the process (the window outlives our worker use).
    let backend = unsafe { Backend::from_foreign_display(display.cast()) };
    let conn = Connection::from_backend(backend);
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: wl_data_device_manager::WlDataDeviceManager = globals.bind(&qh, 1..=3, ()).map_err(|e| e.to_string())?;
    let manager_version = manager.version();
    let (tx, rx) = channel();
    let (pen_tx, pen_rx) = channel();
    let mut state = State {
        conn: conn.clone(),
        surface: surface as usize,
        manager,
        manager_version,
        seats: Vec::new(),
        offers: HashMap::new(),
        current: None,
        selection: None,
        pos: egui::Pos2::ZERO,
        mods: 0,
        keyboard_focus: false,
        keyboard_flag,
        tx,
        ctx,
        tools: Vec::new(),
        cursor_shapes: None,
        pen_tx,
    };
    globals.contents().with_list(|list| {
        for g in list {
            if g.interface == wl_seat::WlSeat::interface().name {
                let seat: wl_seat::WlSeat = globals.registry().bind(g.name, g.version.min(5), &qh, ());
                state.seats.push(Seat { seat, device: None, keyboard: None });
            }
        }
    });
    for s in &mut state.seats {
        s.device = Some(state.manager.get_data_device(&s.seat, &qh, ()));
    }
    let mut pen_rx = Some(pen_rx);
    if tablet {
        let seats: Vec<wl_seat::WlSeat> = state.seats.iter().map(|s| s.seat.clone()).collect();
        let (manager, cursor) = super::tablet_wayland::bind(&globals, &qh, &seats);
        state.cursor_shapes = cursor;
        if manager.is_none() {
            pen_rx = None;
        }
    } else {
        pen_rx = None;
    }
    std::thread::Builder::new()
        .name("wayland-dnd".into())
        .spawn(move || {
            loop {
                if let Err(e) = queue.blocking_dispatch(&mut state) {
                    eprintln!("mtzd-vector: Wayland DnD worker stopped: {e}");
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((rx, pen_rx))
}

fn pick_mime(offered: &[String]) -> Option<String> {
    ACCEPT.iter().find(|m| offered.iter().any(|o| o == *m)).map(|m| m.to_string())
}

/// Receive one MIME type from an offer (blocking read on the calling thread).
fn fetch(conn: &Connection, offer: &wl_data_offer::WlDataOffer, mime: &str) -> Option<Vec<u8>> {
    let (mut read, write) = std::io::pipe().ok()?;
    offer.receive(mime.to_string(), write.as_fd());
    drop(write);
    conn.flush().ok()?;
    let mut buf = Vec::new();
    read.read_to_end(&mut buf).ok()?;
    Some(buf)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for State {
    fn event(_: &mut Self, _: &wl_data_device_manager::WlDataDeviceManager, _: wl_data_device_manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
            let Some(s) = state.seats.iter_mut().find(|s| &s.seat == seat) else { return };
            if caps.contains(wl_seat::Capability::Keyboard) && s.keyboard.is_none() {
                s.keyboard = Some(seat.get_keyboard(qh, ()));
                state.keyboard_flag.store(true, Ordering::Relaxed);
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => {
                state.keyboard_focus = surface.id().as_ptr() as usize == state.surface;
            }
            wl_keyboard::Event::Leave { .. } => state.keyboard_focus = false,
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, .. } => state.mods = mods_depressed | mods_latched,
            wl_keyboard::Event::Key { key, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } if state.keyboard_focus => {
                // Standard xkb modifier masks: Shift=1, Control=4, Mod1(Alt)=8.
                let ctrl = state.mods & 4 != 0;
                if !ctrl {
                    return;
                }
                // evdev key codes.
                let k = match key {
                    46 => ClipKey::Copy,
                    45 => ClipKey::Cut,
                    47 => ClipKey::Paste,
                    _ => return,
                };
                let mods = egui::Modifiers { ctrl: true, command: true, shift: state.mods & 1 != 0, alt: state.mods & 8 != 0, mac_cmd: false };
                state.send(PlatformEvent::Key { key: k, mods });
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(state: &mut Self, _: &wl_data_device::WlDataDevice, event: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use wl_data_device::Event;
        match event {
            Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            Event::Enter { serial, surface, x, y, id } => {
                let Some(offer) = id else { return };
                if surface.id().as_ptr() as usize != state.surface {
                    offer.destroy();
                    state.offers.remove(&offer.id());
                    return;
                }
                let mime = state.offers.get(&offer.id()).and_then(|m| pick_mime(m));
                offer.accept(serial, mime.clone());
                if state.manager_version >= 3 {
                    let copy = wl_data_device_manager::DndAction::Copy;
                    offer.set_actions(copy, copy);
                }
                state.pos = egui::pos2(x as f32, y as f32);
                if mime.is_some() {
                    state.send(PlatformEvent::DragHover(state.pos));
                }
                state.current = Some((offer, serial, mime));
            }
            Event::Motion { x, y, .. } => {
                state.pos = egui::pos2(x as f32, y as f32);
                if let Some((offer, serial, mime)) = &state.current {
                    offer.accept(*serial, mime.clone());
                    if mime.is_some() {
                        state.send(PlatformEvent::DragHover(state.pos));
                    }
                }
            }
            Event::Leave => {
                if let Some((offer, _, _)) = state.current.take() {
                    state.offers.remove(&offer.id());
                    offer.destroy();
                }
                state.send(PlatformEvent::DragLeave);
            }
            Event::Drop => {
                let Some((offer, _, Some(_))) = state.current.take() else {
                    state.send(PlatformEvent::DragLeave);
                    return;
                };
                let offered = state.offers.remove(&offer.id()).unwrap_or_default();
                let conn = state.conn.clone();
                let tx = state.tx.clone();
                let ctx = state.ctx.clone();
                let pos = state.pos;
                let v3 = state.manager_version >= 3;
                let has = |m: &str| offered.iter().any(|o| o == m);
                let image_mime = ACCEPT[1..].iter().find(|m| has(m)).map(|m| m.to_string());
                let has_uris = has("text/uri-list");
                // Read on a helper thread: the source writes asynchronously and
                // must not block our event queue.
                std::thread::spawn(move || {
                    let mut ev = None;
                    if has_uris {
                        if let Some(b) = fetch(&conn, &offer, "text/uri-list") {
                            let files = super::parse_uri_list(&String::from_utf8_lossy(&b));
                            if !files.is_empty() {
                                ev = Some(PlatformEvent::DropFiles(files, pos));
                            }
                        }
                    }
                    if ev.is_none() {
                        if let Some(m) = image_mime {
                            if let Some(bytes) = fetch(&conn, &offer, &m) {
                                ev = Some(PlatformEvent::DropData { mime: m, bytes, pos });
                            }
                        }
                    }
                    if v3 {
                        offer.finish();
                    }
                    offer.destroy();
                    let _ = conn.flush();
                    let _ = tx.send(ev.unwrap_or(PlatformEvent::DragLeave));
                    ctx.request_repaint();
                });
            }
            Event::Selection { id } => {
                // Clipboard offers: we read the clipboard through data-control,
                // so just keep the bookkeeping tidy.
                if let Some(old) = state.selection.take() {
                    state.offers.remove(&old.id());
                    old.destroy();
                }
                state.selection = id;
            }
            _ => {}
        }
    }

    event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for State {
    fn event(state: &mut Self, offer: &wl_data_offer::WlDataOffer, event: wl_data_offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.offers.entry(offer.id()).or_default().push(mime_type);
        }
    }
}
