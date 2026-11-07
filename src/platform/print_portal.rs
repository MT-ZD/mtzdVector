//! Printing through the XDG desktop portal (`org.freedesktop.portal.Print`):
//! the desktop's print dialog, then the PDF handed over as a file
//! descriptor. Works inside Flatpak. libdbus is loaded at run time (as rfd
//! does for file dialogs), so there is no build or link dependency.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

const BUS_SESSION: c_int = 0;
const T_STRING: c_int = b's' as c_int;
const T_OBJECT_PATH: c_int = b'o' as c_int;
const T_UINT32: c_int = b'u' as c_int;
const T_BOOLEAN: c_int = b'b' as c_int;
const T_UNIX_FD: c_int = b'h' as c_int;
const T_ARRAY: c_int = b'a' as c_int;
const T_VARIANT: c_int = b'v' as c_int;
const T_DICT_ENTRY: c_int = b'e' as c_int;

#[repr(C)]
struct DBusError {
    name: *const c_char,
    message: *const c_char,
    dummy: c_uint,
    padding: *mut c_void,
}

/// Opaque iterator (72 bytes in libdbus on 64-bit; room to spare).
#[repr(C)]
#[derive(Clone, Copy)]
struct Iter([u64; 16]);

impl Iter {
    fn new() -> Self {
        Iter([0; 16])
    }
}

type Conn = c_void;
type Msg = c_void;

macro_rules! dbus_fns {
    ($($name:ident: fn($($arg:ty),*) -> $ret:ty;)*) => {
        #[allow(non_snake_case)]
        struct Lib {
            $($name: unsafe extern "C" fn($($arg),*) -> $ret,)*
        }
        impl Lib {
            fn load() -> Result<Lib, String> {
                let handle = ["libdbus-1.so.3", "libdbus-1.so"].iter().find_map(|n| {
                    let c = CString::new(*n).ok()?;
                    let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
                    (!h.is_null()).then_some(h)
                }).ok_or("libdbus is not available")?;
                Ok(Lib {
                    $($name: unsafe {
                        let sym = libc::dlsym(handle, concat!(stringify!($name), "\0").as_ptr() as *const c_char);
                        if sym.is_null() {
                            return Err(format!("libdbus lacks {}", stringify!($name)));
                        }
                        std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) -> $ret>(sym)
                    },)*
                })
            }
        }
    };
}

dbus_fns! {
    dbus_error_init: fn(*mut DBusError) -> ();
    dbus_error_is_set: fn(*const DBusError) -> u32;
    dbus_error_free: fn(*mut DBusError) -> ();
    dbus_bus_get: fn(c_int, *mut DBusError) -> *mut Conn;
    dbus_bus_get_unique_name: fn(*mut Conn) -> *const c_char;
    dbus_bus_add_match: fn(*mut Conn, *const c_char, *mut DBusError) -> ();
    dbus_connection_unref: fn(*mut Conn) -> ();
    dbus_connection_can_send_type: fn(*mut Conn, c_int) -> u32;
    dbus_connection_send_with_reply_and_block: fn(*mut Conn, *mut Msg, c_int, *mut DBusError) -> *mut Msg;
    dbus_connection_read_write: fn(*mut Conn, c_int) -> u32;
    dbus_connection_pop_message: fn(*mut Conn) -> *mut Msg;
    dbus_message_new_method_call: fn(*const c_char, *const c_char, *const c_char, *const c_char) -> *mut Msg;
    dbus_message_unref: fn(*mut Msg) -> ();
    dbus_message_is_signal: fn(*mut Msg, *const c_char, *const c_char) -> u32;
    dbus_message_get_path: fn(*mut Msg) -> *const c_char;
    dbus_message_iter_init: fn(*mut Msg, *mut Iter) -> u32;
    dbus_message_iter_init_append: fn(*mut Msg, *mut Iter) -> ();
    dbus_message_iter_append_basic: fn(*mut Iter, c_int, *const c_void) -> u32;
    dbus_message_iter_open_container: fn(*mut Iter, c_int, *const c_char, *mut Iter) -> u32;
    dbus_message_iter_close_container: fn(*mut Iter, *mut Iter) -> u32;
    dbus_message_iter_get_arg_type: fn(*mut Iter) -> c_int;
    dbus_message_iter_get_basic: fn(*mut Iter, *mut c_void) -> ();
    dbus_message_iter_next: fn(*mut Iter) -> u32;
    dbus_message_iter_recurse: fn(*mut Iter, *mut Iter) -> ();
}

/// A value in an `a{sv}` options dictionary.
enum Val<'a> {
    Str(&'a str),
    U32(u32),
    Bool(bool),
}

struct Bus {
    lib: Lib,
    conn: *mut Conn,
}

impl Drop for Bus {
    fn drop(&mut self) {
        unsafe { (self.lib.dbus_connection_unref)(self.conn) };
    }
}

fn cs(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Bus {
    fn session() -> Result<Bus, String> {
        let lib = Lib::load()?;
        let mut err = DBusError { name: std::ptr::null(), message: std::ptr::null(), dummy: 0, padding: std::ptr::null_mut() };
        unsafe { (lib.dbus_error_init)(&mut err) };
        let conn = unsafe { (lib.dbus_bus_get)(BUS_SESSION, &mut err) };
        let bus = Bus { lib, conn };
        bus.check(&mut err)?;
        if conn.is_null() {
            return Err("No session bus".into());
        }
        Ok(bus)
    }

    fn check(&self, err: &mut DBusError) -> Result<(), String> {
        unsafe {
            if (self.lib.dbus_error_is_set)(err) != 0 {
                let name = if err.name.is_null() { String::new() } else { CStr::from_ptr(err.name).to_string_lossy().into_owned() };
                let msg = if err.message.is_null() { String::new() } else { CStr::from_ptr(err.message).to_string_lossy().into_owned() };
                (self.lib.dbus_error_free)(err);
                return Err(if msg.is_empty() { name } else { format!("{name}: {msg}") });
            }
        }
        Ok(())
    }

    fn new_err(&self) -> DBusError {
        let mut err = DBusError { name: std::ptr::null(), message: std::ptr::null(), dummy: 0, padding: std::ptr::null_mut() };
        unsafe { (self.lib.dbus_error_init)(&mut err) };
        err
    }

    /// The request object path the portal will use for `token`.
    fn request_path(&self, token: &str) -> String {
        let unique = unsafe { CStr::from_ptr((self.lib.dbus_bus_get_unique_name)(self.conn)) }.to_string_lossy().into_owned();
        let sender = unique.trim_start_matches(':').replace('.', "_");
        format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
    }

    fn watch(&self, path: &str) -> Result<(), String> {
        let rule = cs(&format!("type='signal',interface='org.freedesktop.portal.Request',member='Response',path='{path}'"));
        let mut err = self.new_err();
        unsafe { (self.lib.dbus_bus_add_match)(self.conn, rule.as_ptr(), &mut err) };
        self.check(&mut err)
    }

    fn append_str(&self, it: &mut Iter, s: &str) {
        let c = cs(s);
        let p = c.as_ptr();
        unsafe { (self.lib.dbus_message_iter_append_basic)(it, T_STRING, &p as *const _ as *const c_void) };
    }

    fn append_dict(&self, it: &mut Iter, entries: &[(&str, Val)]) {
        let l = &self.lib;
        let sig = cs("{sv}");
        let mut arr = Iter::new();
        unsafe {
            (l.dbus_message_iter_open_container)(it, T_ARRAY, sig.as_ptr(), &mut arr);
            for (k, v) in entries {
                let mut e = Iter::new();
                (l.dbus_message_iter_open_container)(&mut arr, T_DICT_ENTRY, std::ptr::null(), &mut e);
                self.append_str(&mut e, k);
                let vsig = cs(match v {
                    Val::Str(_) => "s",
                    Val::U32(_) => "u",
                    Val::Bool(_) => "b",
                });
                let mut var = Iter::new();
                (l.dbus_message_iter_open_container)(&mut e, T_VARIANT, vsig.as_ptr(), &mut var);
                match v {
                    Val::Str(s) => self.append_str(&mut var, s),
                    Val::U32(u) => {
                        (l.dbus_message_iter_append_basic)(&mut var, T_UINT32, u as *const u32 as *const c_void);
                    }
                    Val::Bool(b) => {
                        let b = *b as u32;
                        (l.dbus_message_iter_append_basic)(&mut var, T_BOOLEAN, &b as *const u32 as *const c_void);
                    }
                }
                (l.dbus_message_iter_close_container)(&mut e, &mut var);
                (l.dbus_message_iter_close_container)(&mut arr, &mut e);
            }
            (l.dbus_message_iter_close_container)(it, &mut arr);
        }
    }

    /// Call a portal method; `fill` appends the arguments. Returns the
    /// reply's first object path (the request handle), if any.
    fn call(&self, iface: &str, method: &str, fill: impl FnOnce(&Bus, &mut Iter)) -> Result<Option<String>, String> {
        let l = &self.lib;
        let (dest, path, iface, method) = (cs("org.freedesktop.portal.Desktop"), cs("/org/freedesktop/portal/desktop"), cs(iface), cs(method));
        let msg = unsafe { (l.dbus_message_new_method_call)(dest.as_ptr(), path.as_ptr(), iface.as_ptr(), method.as_ptr()) };
        if msg.is_null() {
            return Err("Out of memory".into());
        }
        let mut it = Iter::new();
        unsafe { (l.dbus_message_iter_init_append)(msg, &mut it) };
        fill(self, &mut it);
        let mut err = self.new_err();
        let reply = unsafe { (l.dbus_connection_send_with_reply_and_block)(self.conn, msg, 30_000, &mut err) };
        unsafe { (l.dbus_message_unref)(msg) };
        self.check(&mut err)?;
        if reply.is_null() {
            return Err("No reply from the portal".into());
        }
        let mut out = None;
        unsafe {
            let mut it = Iter::new();
            if (l.dbus_message_iter_init)(reply, &mut it) != 0 && (l.dbus_message_iter_get_arg_type)(&mut it) == T_OBJECT_PATH {
                let mut p: *const c_char = std::ptr::null();
                (l.dbus_message_iter_get_basic)(&mut it, &mut p as *mut _ as *mut c_void);
                if !p.is_null() {
                    out = Some(CStr::from_ptr(p).to_string_lossy().into_owned());
                }
            }
            (l.dbus_message_unref)(reply);
        }
        Ok(out)
    }

    /// Wait for the Response signal of the request at `path`: (response
    /// code, the `token` result if any). 0 = success, 1 = cancelled.
    fn wait_response(&self, path: &str, timeout: Duration) -> Result<(u32, Option<u32>), String> {
        let l = &self.lib;
        let (iface, member) = (cs("org.freedesktop.portal.Request"), cs("Response"));
        let start = Instant::now();
        while start.elapsed() < timeout {
            if unsafe { (l.dbus_connection_read_write)(self.conn, 250) } == 0 {
                return Err("Lost the session bus".into());
            }
            loop {
                let msg = unsafe { (l.dbus_connection_pop_message)(self.conn) };
                if msg.is_null() {
                    break;
                }
                let hit = unsafe { (l.dbus_message_is_signal)(msg, iface.as_ptr(), member.as_ptr()) != 0 && {
                    let p = (l.dbus_message_get_path)(msg);
                    !p.is_null() && CStr::from_ptr(p).to_bytes() == path.as_bytes()
                } };
                let result = hit.then(|| unsafe { self.parse_response(msg) });
                unsafe { (l.dbus_message_unref)(msg) };
                if let Some(r) = result {
                    return Ok(r);
                }
            }
        }
        Err("The print dialog timed out".into())
    }

    unsafe fn parse_response(&self, msg: *mut Msg) -> (u32, Option<u32>) {
        let l = &self.lib;
        unsafe {
            let mut it = Iter::new();
            let mut code = 2u32;
            let mut token = None;
            if (l.dbus_message_iter_init)(msg, &mut it) == 0 {
                return (code, None);
            }
            if (l.dbus_message_iter_get_arg_type)(&mut it) == T_UINT32 {
                (l.dbus_message_iter_get_basic)(&mut it, &mut code as *mut u32 as *mut c_void);
            }
            if (l.dbus_message_iter_next)(&mut it) != 0 && (l.dbus_message_iter_get_arg_type)(&mut it) == T_ARRAY {
                let mut arr = Iter::new();
                (l.dbus_message_iter_recurse)(&mut it, &mut arr);
                while (l.dbus_message_iter_get_arg_type)(&mut arr) == T_DICT_ENTRY {
                    let mut e = Iter::new();
                    (l.dbus_message_iter_recurse)(&mut arr, &mut e);
                    let mut k: *const c_char = std::ptr::null();
                    if (l.dbus_message_iter_get_arg_type)(&mut e) == T_STRING {
                        (l.dbus_message_iter_get_basic)(&mut e, &mut k as *mut _ as *mut c_void);
                    }
                    if !k.is_null() && CStr::from_ptr(k).to_bytes() == b"token" && (l.dbus_message_iter_next)(&mut e) != 0 {
                        let mut v = Iter::new();
                        (l.dbus_message_iter_recurse)(&mut e, &mut v);
                        if (l.dbus_message_iter_get_arg_type)(&mut v) == T_UINT32 {
                            let mut t = 0u32;
                            (l.dbus_message_iter_get_basic)(&mut v, &mut t as *mut u32 as *mut c_void);
                            token = Some(t);
                        }
                    }
                    if (l.dbus_message_iter_next)(&mut arr) == 0 {
                        break;
                    }
                }
            }
            (code, token)
        }
    }
}

/// Show the desktop print dialog and print the PDF at `path`. Ok(false)
/// when the user cancels. Blocks until the dialog closes (run it on a
/// thread).
pub fn print_pdf(path: &Path, title: &str) -> Result<bool, String> {
    let bus = Bus::session()?;
    if unsafe { (bus.lib.dbus_connection_can_send_type)(bus.conn, T_UNIX_FD) } == 0 {
        return Err("The session bus can't pass files".into());
    }
    let n = std::process::id();
    let t1 = format!("mtzd_prepare_{n}");
    let p1 = bus.request_path(&t1);
    bus.watch(&p1)?;
    let handle = bus.call("org.freedesktop.portal.Print", "PreparePrint", |b, it| {
        b.append_str(it, "");
        b.append_str(it, title);
        b.append_dict(it, &[]);
        b.append_dict(it, &[]);
        b.append_dict(it, &[("handle_token", Val::Str(&t1)), ("modal", Val::Bool(true))]);
    })?;
    let p1 = match handle {
        Some(h) if h != p1 => {
            bus.watch(&h)?;
            h
        }
        _ => p1,
    };
    let (code, token) = bus.wait_response(&p1, Duration::from_secs(3600))?;
    if code != 0 {
        return Ok(false);
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let fd: c_int = file.as_raw_fd();
    let t2 = format!("mtzd_print_{n}");
    let p2 = bus.request_path(&t2);
    bus.watch(&p2)?;
    let mut opts = vec![("handle_token", Val::Str(&t2)), ("modal", Val::Bool(true))];
    if let Some(t) = token {
        opts.push(("token", Val::U32(t)));
    }
    let handle = bus.call("org.freedesktop.portal.Print", "Print", |b, it| {
        b.append_str(it, "");
        b.append_str(it, title);
        unsafe { (b.lib.dbus_message_iter_append_basic)(it, T_UNIX_FD, &fd as *const c_int as *const c_void) };
        b.append_dict(it, &opts);
    })?;
    let p2 = match handle {
        Some(h) if h != p2 => {
            bus.watch(&h)?;
            h
        }
        _ => p2,
    };
    let (code, _) = bus.wait_response(&p2, Duration::from_secs(600))?;
    drop(file);
    Ok(code == 0)
}

/// The Print portal's interface version (a harmless call that shows no UI),
/// or why it can't be reached.
pub fn portal_version() -> Result<u32, String> {
    let bus = Bus::session()?;
    let l = &bus.lib;
    let (dest, path, iface, method) = (cs("org.freedesktop.portal.Desktop"), cs("/org/freedesktop/portal/desktop"), cs("org.freedesktop.DBus.Properties"), cs("Get"));
    let msg = unsafe { (l.dbus_message_new_method_call)(dest.as_ptr(), path.as_ptr(), iface.as_ptr(), method.as_ptr()) };
    let mut it = Iter::new();
    unsafe { (l.dbus_message_iter_init_append)(msg, &mut it) };
    bus.append_str(&mut it, "org.freedesktop.portal.Print");
    bus.append_str(&mut it, "version");
    let mut err = bus.new_err();
    let reply = unsafe { (l.dbus_connection_send_with_reply_and_block)(bus.conn, msg, 5_000, &mut err) };
    unsafe { (l.dbus_message_unref)(msg) };
    bus.check(&mut err)?;
    if reply.is_null() {
        return Err("No reply".into());
    }
    let mut v = 0u32;
    unsafe {
        let mut it = Iter::new();
        (l.dbus_message_iter_init)(reply, &mut it);
        if (l.dbus_message_iter_get_arg_type)(&mut it) == T_VARIANT {
            let mut inner = Iter::new();
            (l.dbus_message_iter_recurse)(&mut it, &mut inner);
            if (l.dbus_message_iter_get_arg_type)(&mut inner) == T_UINT32 {
                (l.dbus_message_iter_get_basic)(&mut inner, &mut v as *mut u32 as *mut c_void);
            }
        }
        (l.dbus_message_unref)(reply);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    /// Reads the portal's version over D-Bus (no dialog). Skips without a
    /// session bus or portal.
    #[test]
    fn portal_reachable_or_skipped() {
        match super::portal_version() {
            Ok(v) => assert!(v >= 1, "version {v}"),
            Err(e) => eprintln!("print portal not reachable: {e}"),
        }
    }
}
