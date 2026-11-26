//! A connection to a controller, run on its own thread: it finds the baud
//! rate with a read-only probe, polls status and position, sends jobs and
//! machine commands, and reports everything as events. `stop()` goes ahead
//! of anything queued.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cmd::{Command, Rapid, mem, parse};
use crate::codec::{swizzle, unswizzle};
use crate::job::Pt;
use crate::transport::Transport;

/// Opens the link at a baud rate.
pub type Opener = Box<dyn FnMut(u32) -> io::Result<Box<dyn Transport>> + Send>;

#[derive(Clone, Debug)]
pub struct Config {
    /// Rates to try, in order (the first that answers is kept).
    pub bauds: Vec<u32>,
    pub magic: u8,
    /// Status / position reads while idle.
    pub poll: Duration,
    pub reply_timeout: Duration,
    /// Bytes per write while sending a job.
    pub chunk: usize,
    /// Poll failures in a row before giving up on the link.
    pub max_misses: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { bauds: vec![38400, 115200, 19200], magic: crate::codec::MAGIC_644X, poll: Duration::from_millis(250), reply_timeout: Duration::from_millis(400), chunk: 256, max_misses: 8 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Move the head (laser off), with the red pointer when `light`.
    Rapid { to: Pt, light: bool },
    Home,
    /// Trace these points with the laser off, waiting at each.
    Frame { points: Vec<Pt>, light: bool },
    /// A plain `.rd` stream (it starts itself).
    Job { stream: Vec<u8>, name: String },
    Pause,
    Resume,
    Disconnect,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// The status word as read (its bits aren't fully known).
    pub raw: u64,
    pub running: bool,
    pub pos: Pt,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Connected { link: String, baud: u32, card_id: u64 },
    Status(Status),
    Progress { sent: usize, total: usize },
    /// The whole job went out; the controller is working on it.
    JobSent { name: String },
    /// A finished frame trace.
    Framed,
    Stopped,
    /// tx: sent by us.
    Log { tx: bool, text: String },
    Error(String),
    Disconnected,
}

pub struct Session {
    tx: Sender<Request>,
    pub events: Receiver<Event>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
}

impl Session {
    pub fn start(opener: Opener, cfg: Config, wake: Arc<dyn Fn() + Send + Sync>) -> Session {
        let (tx, rx) = mpsc::channel();
        let (etx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let w = Worker { opener, cfg, rx, tx: etx, wake, stop: stop.clone(), link: None, last_pos: (0, 0), quit: false };
        let a = alive.clone();
        std::thread::Builder::new()
            .name("ruida".into())
            .spawn(move || {
                let mut w = w;
                w.run();
                a.store(false, Ordering::SeqCst);
                w.emit(Event::Disconnected);
            })
            .expect("thread");
        Session { tx, events, stop, alive }
    }

    pub fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    /// Stop the job now (ahead of anything queued, mid-upload too).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Disconnect);
    }
}

struct Worker {
    opener: Opener,
    cfg: Config,
    rx: Receiver<Request>,
    tx: Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    stop: Arc<AtomicBool>,
    link: Option<Box<dyn Transport>>,
    last_pos: Pt,
    quit: bool,
}

enum Fail {
    Io(io::Error),
    NoReply,
    Stopped,
}

impl Worker {
    fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
        (self.wake)();
    }

    fn log(&self, tx: bool, text: impl Into<String>) {
        self.emit(Event::Log { tx, text: text.into() });
    }

    fn run(&mut self) {
        if !self.connect() {
            return;
        }
        let mut misses = 0;
        loop {
            if self.check_stop() || self.quit {
                if self.quit {
                    return;
                }
                continue;
            }
            match self.rx.recv_timeout(self.cfg.poll) {
                Ok(Request::Disconnect) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(r) => {
                    if let Err(e) = self.handle(r) {
                        match e {
                            Fail::Stopped => {}
                            Fail::Io(e) => {
                                self.emit(Event::Error(format!("link error: {e}")));
                                return;
                            }
                            Fail::NoReply => self.emit(Event::Error("the controller stopped answering".into())),
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => match self.status() {
                    Ok(s) => {
                        misses = 0;
                        self.emit(Event::Status(s));
                    }
                    Err(Fail::Io(e)) => {
                        self.emit(Event::Error(format!("link error: {e}")));
                        return;
                    }
                    Err(_) => {
                        misses += 1;
                        if misses >= self.cfg.max_misses {
                            self.emit(Event::Error("no answer from the controller (cable unplugged or controller off?)".into()));
                            return;
                        }
                    }
                },
            }
        }
    }

    /// Try each baud rate with a harmless memory read (the card id).
    fn connect(&mut self) -> bool {
        let bauds = self.cfg.bauds.clone();
        for baud in bauds {
            let t = match (self.opener)(baud) {
                Ok(t) => t,
                Err(e) => {
                    let hint = if e.kind() == io::ErrorKind::PermissionDenied {
                        " — your user may need to be in the serial group (uucp on Arch, dialout on Debian/Ubuntu); log in again after adding it"
                    } else {
                        ""
                    };
                    self.emit(Event::Error(format!("can't open the port: {e}{hint}")));
                    return false;
                }
            };
            let link = t.describe();
            self.link = Some(t);
            self.log(true, format!("probe {link}: read card id"));
            match self.read_mem(mem::CARD_ID) {
                Ok(card_id) => {
                    self.log(false, format!("card id 0x{card_id:X}"));
                    self.emit(Event::Connected { link, baud, card_id });
                    return true;
                }
                Err(Fail::Io(e)) => {
                    self.emit(Event::Error(format!("link error: {e}")));
                    return false;
                }
                Err(_) => self.log(false, format!("no answer at {baud} baud")),
            }
            self.link = None;
        }
        self.emit(Event::Error("the controller didn't answer at any baud rate (is it on? the right port?)".into()));
        false
    }

    fn write_all(&mut self, plain: &[u8], interruptible: bool) -> Result<(), Fail> {
        let data = swizzle(plain, self.cfg.magic);
        let mut at = 0;
        let mut idle = Instant::now();
        while at < data.len() {
            if interruptible && self.stop.load(Ordering::SeqCst) {
                return Err(Fail::Stopped);
            }
            let t = self.link.as_mut().ok_or(Fail::NoReply)?;
            let n = t.write(&data[at..]).map_err(Fail::Io)?;
            if n == 0 {
                // Held back by flow control: the controller is busy.
                if idle.elapsed() > Duration::from_secs(120) {
                    return Err(Fail::NoReply);
                }
                continue;
            }
            idle = Instant::now();
            at += n;
        }
        Ok(())
    }

    fn send(&mut self, c: &Command) -> Result<(), Fail> {
        self.log(true, c.describe());
        self.write_all(&c.encode(), false)
    }

    /// Read a memory value (`DA 00 <addr>` → `DA 01 <addr> <value>`).
    fn read_mem(&mut self, addr: u16) -> Result<u64, Fail> {
        self.write_all(&Command::ReadMem(addr).encode(), false)?;
        let deadline = Instant::now() + self.cfg.reply_timeout;
        let mut got = Vec::new();
        let mut buf = [0u8; 64];
        while Instant::now() < deadline {
            let t = self.link.as_mut().ok_or(Fail::NoReply)?;
            let n = t.read(&mut buf, Duration::from_millis(50)).map_err(Fail::Io)?;
            got.extend_from_slice(&buf[..n]);
            for c in parse(&unswizzle(&got, self.cfg.magic)) {
                if let Command::MemValue { addr: a, value } = c
                    && a == addr
                {
                    return Ok(value);
                }
            }
        }
        if !got.is_empty() {
            self.log(false, format!("unexpected reply: {}", crate::cmd::hex(&unswizzle(&got, self.cfg.magic))));
        }
        Err(Fail::NoReply)
    }

    fn status(&mut self) -> Result<Status, Fail> {
        let raw = self.read_mem(mem::MACHINE_STATUS)?;
        let x = self.read_mem(mem::POS_X)?;
        let y = self.read_mem(mem::POS_Y)?;
        let s35 = |v: u64| crate::codec::snum35(&crate::codec::num35(v as i64));
        let pos = (s35(x), s35(y));
        self.last_pos = pos;
        Ok(Status { raw, running: raw & 0x01 != 0, pos })
    }

    /// A pending stop: send it now.
    fn check_stop(&mut self) -> bool {
        if !self.stop.swap(false, Ordering::SeqCst) {
            return false;
        }
        // Drop queued motion; keep a disconnect.
        while let Ok(r) = self.rx.try_recv() {
            self.quit |= r == Request::Disconnect;
        }
        match self.send(&Command::StopProcess) {
            Ok(()) => self.emit(Event::Stopped),
            Err(_) => self.emit(Event::Error("couldn't send STOP: use the machine's stop button".into())),
        }
        true
    }

    fn wait_at(&mut self, to: Pt) -> Result<(), Fail> {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if self.stop.load(Ordering::SeqCst) {
                return Err(Fail::Stopped);
            }
            std::thread::sleep(self.cfg.poll);
            // Homing and long moves can keep it from answering for a while.
            if let Ok(s) = self.status() {
                self.emit(Event::Status(s));
                if (s.pos.0 - to.0).abs() < 100 && (s.pos.1 - to.1).abs() < 100 {
                    return Ok(());
                }
            }
        }
        Err(Fail::NoReply)
    }

    fn handle(&mut self, r: Request) -> Result<(), Fail> {
        match r {
            Request::Rapid { to, light } => self.send(&Command::RapidXY { opt: if light { Rapid::Light } else { Rapid::Plain }, x: to.0, y: to.1 }),
            Request::Home => self.send(&Command::HomeXY),
            Request::Frame { points, light } => {
                for p in points {
                    let r = self.send(&Command::RapidXY { opt: if light { Rapid::Light } else { Rapid::Plain }, x: p.0, y: p.1 });
                    r.and_then(|_| self.wait_at(p))?;
                }
                self.emit(Event::Framed);
                Ok(())
            }
            Request::Pause => self.send(&Command::PauseProcess),
            Request::Resume => self.send(&Command::ResumeProcess),
            Request::Job { stream, name } => {
                let n = parse(&stream).len();
                self.log(true, format!("job \"{name}\": {} bytes, {n} commands", stream.len()));
                let total = stream.len();
                let mut sent = 0;
                // Chunks end between commands: a stop never lands inside one
                // (a controller reading fixed-length parameters would take
                // the stop's bytes as coordinates).
                let mut chunks: Vec<&[u8]> = Vec::new();
                let (mut start, mut end) = (0, 0);
                for c in crate::cmd::split(&stream) {
                    let len = c.len();
                    if end > start && end - start + len > self.cfg.chunk.max(16) {
                        chunks.push(&stream[start..end]);
                        start = end;
                    }
                    end += len;
                }
                if end > start {
                    chunks.push(&stream[start..end]);
                }
                for chunk in chunks {
                    if let Err(e) = self.write_all(chunk, true) {
                        if matches!(e, Fail::Stopped) {
                            self.stop.store(true, Ordering::SeqCst);
                            self.check_stop();
                        }
                        return Err(e);
                    }
                    sent += chunk.len();
                    self.emit(Event::Progress { sent, total });
                }
                self.emit(Event::JobSent { name });
                Ok(())
            }
            Request::Disconnect => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::{EmulatedTransport, Emulator};
    use crate::job::{Job, JobLayer};
    use std::sync::Mutex;

    fn session(emu: &Arc<Mutex<Emulator>>, cfg: Config) -> Session {
        let e = emu.clone();
        let opener: Opener = Box::new(move |baud| Ok(Box::new(EmulatedTransport { emu: e.clone(), baud }) as Box<dyn Transport>));
        Session::start(opener, cfg, Arc::new(|| {}))
    }

    fn fast() -> Config {
        Config { poll: Duration::from_millis(10), reply_timeout: Duration::from_millis(60), ..Default::default() }
    }

    fn wait(s: &Session, what: impl Fn(&Event) -> bool) -> Vec<Event> {
        let mut seen = Vec::new();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Ok(e) = s.events.recv_timeout(Duration::from_millis(50)) {
                let done = what(&e);
                seen.push(e);
                if done {
                    return seen;
                }
            }
        }
        panic!("timed out; saw {seen:?}");
    }

    #[test]
    fn probe_finds_the_baud_rate_and_polls() {
        let emu = Emulator { baud: 115200, ..Default::default() }.shared();
        let s = session(&emu, fast());
        let ev = wait(&s, |e| matches!(e, Event::Connected { .. }));
        assert!(matches!(ev.last(), Some(Event::Connected { baud: 115200, card_id: 0x6510_6510, .. })));
        // Wrong rates only ever saw a 4-byte read, never motion.
        assert_eq!(emu.lock().unwrap().noise, 4);
        emu.lock().unwrap().pos = (12_000, 34_000);
        wait(&s, |e| matches!(e, Event::Status(st) if st.pos == (12_000, 34_000)));
    }

    #[test]
    fn job_runs_and_stop_goes_first() {
        let emu = Emulator::default().shared();
        let s = session(&emu, fast());
        wait(&s, |e| matches!(e, Event::Connected { .. }));
        let sq = vec![(10_000, 10_000), (30_000, 10_000), (30_000, 30_000), (10_000, 10_000)];
        let job = Job { layers: vec![JobLayer { rgb: [0, 0, 0], speed: 10.0, min_power: 5.0, max_power: 10.0, air: true, passes: 1, paths: vec![sq] }] };
        s.send(Request::Job { stream: job.encode(), name: "t".into() });
        wait(&s, |e| matches!(e, Event::JobSent { .. }));
        assert_eq!(emu.lock().unwrap().cuts.len(), 3);
        wait(&s, |e| matches!(e, Event::Status(st) if !st.running));
        // A big job, stopped while it is being sent.
        let many: Vec<Vec<Pt>> = (0..3000).map(|i| vec![(i, 0), (i, 1000)]).collect();
        let big = Job { layers: vec![JobLayer { paths: many, ..job.layers[0].clone() }] };
        s.send(Request::Job { stream: big.encode(), name: "big".into() });
        wait(&s, |e| matches!(e, Event::Progress { .. }));
        s.stop();
        let ev = wait(&s, |e| matches!(e, Event::Stopped));
        assert!(!ev.iter().any(|e| matches!(e, Event::JobSent { .. })), "{ev:?}");
        wait(&s, |e| matches!(e, Event::Status(_)));
        let e = emu.lock().unwrap();
        // The stop arrived whole, right after a complete command.
        let stop = e.received.iter().position(|c| *c == Command::StopProcess).expect("stop received");
        assert!(e.received[..stop].iter().all(|c| !matches!(c, Command::Other(_))), "{:?}", &e.received[stop.saturating_sub(3)..=stop]);
        assert!(!e.running && e.cuts.len() < 3 + 3000);
    }

    #[test]
    fn frame_waits_for_each_corner() {
        let emu = Emulator::default().shared();
        let s = session(&emu, fast());
        wait(&s, |e| matches!(e, Event::Connected { .. }));
        let pts = vec![(0, 0), (20_000, 0), (20_000, 10_000), (0, 10_000), (0, 0)];
        s.send(Request::Frame { points: pts.clone(), light: true });
        wait(&s, |e| matches!(e, Event::Framed));
        let e = emu.lock().unwrap();
        let rapids: Vec<Pt> = e.received.iter().filter_map(|c| if let Command::RapidXY { x, y, opt: Rapid::Light } = c { Some((*x, *y)) } else { None }).collect();
        assert_eq!(rapids, pts);
        assert!(e.cuts.is_empty());
    }

    #[test]
    fn dead_link_reports_instead_of_hanging() {
        let emu = Emulator { responds: false, ..Default::default() }.shared();
        let s = session(&emu, fast());
        let ev = wait(&s, |e| matches!(e, Event::Disconnected));
        assert!(ev.iter().any(|e| matches!(e, Event::Error(m) if m.contains("baud"))), "{ev:?}");
        assert!(!s.is_alive());
    }
}
