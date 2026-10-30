//! Ruida laser controller support (RDC644x family) over Ethernet.
//!
//! Implemented from public sources: the EduTech Wiki "Ruida" protocol page and
//! the open-source MeerK40t project's Ruida driver.

pub mod codec;
pub mod controller;
pub mod job;
pub mod link;
pub mod rd_file;

pub use codec::RuidaModel;
pub use controller::{Controller, JogKey, MachineState};
pub use job::{encode_job, RdLayer, RdPath};
pub use link::LinkError;
