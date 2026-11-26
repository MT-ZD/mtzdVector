//! A fake RDC644x on the USB link: it unswizzles what it receives, keeps
//! track of position and job state, answers memory reads, and records
//! everything (for tests, and for trying the panel without a machine).

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::cmd::{Command, mem, split};
use crate::codec::{swizzle, unswizzle};
use crate::job::Pt;
use crate::transport::Transport;

pub struct Emulator {
    pub magic: u8,
    /// The baud rate it listens at (other rates read as noise).
    pub baud: u32,
    pub card_id: u64,
    /// Answers at all (false: a dead link).
    pub responds: bool,
    pub pos: Pt,
    pub running: bool,
    pub paused: bool,
    /// Status reads a job stays "running" for after its last byte.
    pub run_polls: u32,
    pub(crate) left: u32,
    pub(crate) pending: Vec<u8>,
    pub(crate) out: VecDeque<u8>,
    /// Every command received, in order.
    pub received: Vec<Command>,
    /// Cuts made: (layer, from, to).
    pub cuts: Vec<(u8, Pt, Pt)>,
    pub(crate) layer: u8,
    /// Bytes received that weren't at its baud rate.
    pub noise: usize,
}

impl Default for Emulator {
    fn default() -> Self {
        Emulator {
            magic: crate::codec::MAGIC_644X,
            baud: 38400,
            card_id: 0x6510_6510,
            responds: true,
            pos: (0, 0),
            running: false,
            paused: false,
            run_polls: 2,
            left: 0,
            pending: Vec::new(),
            out: VecDeque::new(),
            received: Vec::new(),
            cuts: Vec::new(),
            layer: 0,
            noise: 0,
        }
    }
}

impl Emulator {
    pub fn shared(self) -> Arc<Mutex<Emulator>> {
        Arc::new(Mutex::new(self))
    }

    /// Process received commands; the last one only when `all` (it may
    /// still be arriving otherwise).
    fn process(&mut self, all: bool) {
        let bytes = std::mem::take(&mut self.pending);
        let parts = split(&bytes);
        let n = parts.len();
        let mut keep = Vec::new();
        for (i, p) in parts.into_iter().enumerate() {
            if i + 1 == n && !all {
                keep = p.to_vec();
                break;
            }
            self.apply(Command::decode(p));
        }
        self.pending = keep;
    }

    fn reply(&mut self, c: Command) {
        if self.responds {
            self.out.extend(swizzle(&c.encode(), self.magic));
        }
    }

    fn apply(&mut self, c: Command) {
        use Command::*;
        match &c {
            ReadMem(a) => {
                let value = match *a {
                    mem::CARD_ID => self.card_id,
                    mem::MACHINE_STATUS => {
                        if self.running && self.left > 0 {
                            self.left -= 1;
                            if self.left == 0 {
                                self.running = false;
                            }
                        }
                        (self.running as u64) | ((self.running as u64) << 24)
                    }
                    mem::POS_X => self.pos.0 as u64 & ((1 << 35) - 1),
                    mem::POS_Y => self.pos.1 as u64 & ((1 << 35) - 1),
                    mem::BED_X => 600_000,
                    mem::BED_Y => 400_000,
                    _ => 0,
                };
                self.reply(MemValue { addr: *a, value });
            }
            StartProcess => {
                self.running = true;
                self.left = u32::MAX;
            }
            EndOfFile => self.left = self.run_polls.max(1),
            StopProcess => {
                self.running = false;
                self.paused = false;
            }
            PauseProcess => self.paused = true,
            ResumeProcess => self.paused = false,
            HomeXY => self.pos = (0, 0),
            RapidXY { x, y, .. } | MoveAbs { x, y } => self.pos = (*x, *y),
            MoveRel { dx, dy } => self.pos = (self.pos.0 + dx, self.pos.1 + dy),
            LayerNumber(n) => self.layer = *n,
            CutAbs { x, y } => {
                self.cuts.push((self.layer, self.pos, (*x, *y)));
                self.pos = (*x, *y);
            }
            CutRel { dx, dy } => {
                let to = (self.pos.0 + dx, self.pos.1 + dy);
                self.cuts.push((self.layer, self.pos, to));
                self.pos = to;
            }
            _ => {}
        }
        self.received.push(c);
    }
}

/// The emulator as a transport, opened at some baud rate.
pub struct EmulatedTransport {
    pub emu: Arc<Mutex<Emulator>>,
    pub baud: u32,
}

impl Transport for EmulatedTransport {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut e = self.emu.lock().unwrap();
        if self.baud != e.baud {
            e.noise += data.len();
            return Ok(data.len());
        }
        let magic = e.magic;
        e.pending.extend(unswizzle(data, magic));
        e.process(false);
        Ok(data.len())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let n = {
            let mut e = self.emu.lock().unwrap();
            if self.baud == e.baud {
                e.process(true);
            }
            let n = buf.len().min(e.out.len());
            for b in buf.iter_mut().take(n) {
                *b = e.out.pop_front().unwrap();
            }
            n
        };
        if n == 0 {
            std::thread::sleep(timeout.min(Duration::from_millis(5)));
        }
        Ok(n)
    }

    fn describe(&self) -> String {
        format!("emulator at {} baud", self.baud)
    }
}
