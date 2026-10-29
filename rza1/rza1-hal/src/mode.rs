//! Driver modes: a driver built with `new_blocking` is [`Blocking`] and only polls; one built with
//! `new` is [`Async`] and waits on its interrupts.

mod sealed {
    pub trait Sealed {}
}

/// A driver mode: [`Blocking`] or [`Async`].
pub trait Mode: sealed::Sealed {}

/// Polls the hardware; binds no interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Blocking;

/// Waits on its interrupts with `async` methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Async;

impl sealed::Sealed for Blocking {}
impl Mode for Blocking {}
impl sealed::Sealed for Async {}
impl Mode for Async {}
