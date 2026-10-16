//! Elektron Overbridge 2: the wire format ([`proto`]) shared by the host class
//! (`host::overbridge`) and the device class.

pub mod proto;
pub mod stems;
#[cfg(all(feature = "device-overbridge", any(target_os = "none", test)))]
pub mod device;
