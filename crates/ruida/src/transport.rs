//! Byte links to a controller: the USB serial port (an FTDI chip on the
//! RDC644x), or the emulator for tests and dry runs.

use std::io::{self, Read, Write};
use std::time::Duration;

pub trait Transport: Send {
    /// Write some of `data`; returns how much was taken (0 when the link
    /// is busy for now: flow control).
    fn write(&mut self, data: &[u8]) -> io::Result<usize>;
    /// Bytes that arrive within `timeout` (0 = nothing came).
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
    fn describe(&self) -> String;
}

/// How the serial control lines are driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Handshake {
    /// RTS/CTS flow control (the controller paces a long job).
    #[default]
    Hardware,
    /// No flow control; DTR/RTS pulsed once to wake the controller.
    Pulse,
}

pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
    path: String,
    baud: u32,
}

impl SerialTransport {
    pub fn open(path: &str, baud: u32, handshake: Handshake) -> io::Result<Self> {
        let flow = match handshake {
            Handshake::Hardware => serialport::FlowControl::Hardware,
            Handshake::Pulse => serialport::FlowControl::None,
        };
        let mut port = serialport::new(path, baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(flow)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(io_err)?;
        if handshake == Handshake::Pulse {
            // DTR low + RTS high for ~5 ms, then DTR high + RTS low.
            let _ = port.write_data_terminal_ready(false);
            let _ = port.write_request_to_send(true);
            std::thread::sleep(Duration::from_millis(5));
            let _ = port.write_data_terminal_ready(true);
            let _ = port.write_request_to_send(false);
        }
        let _ = port.clear(serialport::ClearBuffer::All);
        Ok(SerialTransport { port, path: path.into(), baud })
    }
}

fn io_err(e: serialport::Error) -> io::Error {
    match e.kind {
        serialport::ErrorKind::Io(k) => io::Error::new(k, e.description),
        serialport::ErrorKind::NoDevice => io::Error::new(io::ErrorKind::NotFound, e.description),
        _ => io::Error::other(e.description),
    }
}

impl Transport for SerialTransport {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let _ = self.port.set_timeout(Duration::from_millis(200));
        match self.port.write(data) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let _ = self.port.set_timeout(timeout);
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn describe(&self) -> String {
        format!("{} at {} baud", self.path, self.baud)
    }
}

/// Serial ports that may be a controller (USB adapters first).
pub fn ports() -> Vec<String> {
    let mut out: Vec<String> = serialport::available_ports().map(|v| v.into_iter().map(|p| p.port_name).collect()).unwrap_or_default();
    // Without udev the list can miss USB adapters: look for them directly.
    if let Ok(dir) = std::fs::read_dir("/dev") {
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("ttyUSB") || name.starts_with("ttyACM") {
                let p = format!("/dev/{name}");
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    // Built-in UARTs (/dev/ttyS*: dozens, mostly not even wired) aren't the
    // USB cable.
    out.retain(|p| !p.starts_with("/dev/ttyS"));
    out.sort_by_key(|p| (!(p.contains("USB") || p.contains("ACM")), p.clone()));
    out
}
