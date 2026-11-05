//! The SD card's errors, the same on every board.

/// Bringing the card up failed ([`Deluge::sd`](crate::Deluge::sd)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SdError {
    /// No card in the slot.
    NoCard,
    /// The card didn't come up, or holds no FAT volume.
    Failed,
}

/// A file operation on the card failed ([`Sd`](crate::Sd), [`File`](super::File)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FatError {
    /// No file of that name in the root directory.
    NotFound,
    /// The card is full.
    Full,
    /// The name is empty, too long, or has a character FAT doesn't allow.
    InvalidName,
    /// The card holds no FAT volume, or its structures are damaged.
    NotFat,
    /// A seek past the end of the file.
    SeekPastEnd,
    /// The card or its driver failed the transfer.
    Card,
}
