//! Elektron Overbridge 2: the wire format ([`proto`]) shared by the host class
//! (`host::overbridge`) and the device class.

#[cfg(all(feature = "device-overbridge", any(target_os = "none", test)))]
pub mod device;
pub mod proto;
pub mod stems;
