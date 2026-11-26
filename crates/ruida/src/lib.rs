//! Ruida laser controllers (RDC644x): the protocol, `.rd` jobs, a USB
//! serial session and an emulator.
//!
//! Sources: the EduTechWiki Ruida page, kkaempf/ruida and meerk40t's ruida
//! module. Parts of the USB link (baud rate, status bits) aren't documented
//! anywhere reliable: the session finds the baud rate with a read-only
//! probe and logs everything it sends and receives.

pub mod cmd;
pub mod codec;
pub mod emulator;
pub mod job;
pub mod session;
pub mod transport;

pub use cmd::Command;
pub use job::{Estimate, Job, JobLayer, Pt};
pub use session::{Config, Event, Request, Session, Status};
