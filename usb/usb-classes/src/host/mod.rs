//! Host-side class drivers, generic over [`UsbHostAllocator`], so they are unit-tested
//! against [`mock::MockAlloc`] under QEMU.
//!
//! [`UsbHostAllocator`]: embassy_usb_driver::host::UsbHostAllocator

#[cfg(feature = "host-midi")]
pub mod midi;
#[cfg(all(test, not(target_os = "none")))]
pub(crate) mod mock;
#[cfg(feature = "host-overbridge")]
pub mod overbridge;
#[cfg(feature = "host-uac")]
pub mod uac;
